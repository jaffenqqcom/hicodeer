// ===== [OHOS PORT] file added by the OHOS port =====
// OHOS has neither a `cpal` backend nor a `libwebrtc` build, so this module is
// compiled instead of `audio_pipeline` on `target_env = "ohos"` (see the module
// selection in `crates/audio/src/audio.rs`). It keeps the same public surface as
// `audio_pipeline` so that the rest of the workspace compiles unchanged.
//
// The implementation talks to the OHOS audio NDK directly:
// - short effects and the audio test panel play through an `OH_AudioRenderer`
//   fed by the mixer below;
// - the audio test panel records through an `OH_AudioCapturer`;
// - device enumeration goes through `OH_AudioRoutingManager`.

use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::future::Future;
use std::os::raw::{c_char, c_int};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, Global};
use rodio::{ChannelCount, Sample, Source};

use crate::ohos_audio_sys as sys;
use crate::{CHANNEL_COUNT, DeviceId, SAMPLE_RATE, Sound};

/// The methods `settings_ui::pages::audio_test_window` relies on to adapt a raw
/// microphone stream. [`InputStream`] already delivers stereo samples at
/// [`SAMPLE_RATE`], so these are identity operations that keep the call site
/// compiling.
pub trait RodioExt: rodio::Source + Sized {
    fn constant_params(
        self,
        _channel_count: ChannelCount,
        _sample_rate: rodio::SampleRate,
    ) -> Self {
        self
    }

    fn constant_samplerate(self, _sample_rate: rodio::SampleRate) -> Self {
        self
    }

    fn possibly_disconnected_channels_to_mono(self) -> Self {
        self
    }
}

impl<S: rodio::Source> RodioExt for S {}

/// How much audio the capture queue may hold before old samples are dropped.
/// Two seconds of stereo 48 kHz audio.
const MAX_CAPTURED_SAMPLES: usize = (SAMPLE_RATE.get() as usize) * 2 * 2;
/// Samples pushed into the mixer per capture/playback chunk.
const CHUNK_SAMPLES: usize = 1024;
/// How long `InputStream::next` waits for captured audio before emitting
/// silence. Short enough that the consumer observes a stop request promptly.
const CAPTURE_QUEUE_WAIT: Duration = Duration::from_millis(100);
/// How long the renderer stays open after the mixer runs dry.
const IDLE_RELEASE_DELAY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Audio;

impl Global for Audio {}

/// One queued piece of audio, interleaved stereo at [`SAMPLE_RATE`].
struct Clip {
    samples: Arc<Vec<i16>>,
    position: usize,
}

/// Plays queued clips one after another; the renderer callback drains it.
#[derive(Default)]
struct Mixer {
    queue: VecDeque<Clip>,
}

impl Mixer {
    fn push(&mut self, samples: Arc<Vec<i16>>) {
        if !samples.is_empty() {
            self.queue.push_back(Clip {
                samples,
                position: 0,
            });
        }
    }

    fn clear(&mut self) {
        self.queue.clear();
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Fills `output` with interleaved stereo samples, zero-padding when the
    /// queue runs dry.
    fn fill(&mut self, output: &mut [i16]) {
        for frame in output.chunks_exact_mut(2) {
            loop {
                let Some(clip) = self.queue.front_mut() else {
                    frame.fill(0);
                    break;
                };
                if clip.position + 1 >= clip.samples.len() {
                    self.queue.pop_front();
                    continue;
                }
                frame[0] = clip.samples[clip.position];
                frame[1] = clip.samples[clip.position + 1];
                clip.position += 2;
                break;
            }
        }
    }
}

fn mixer() -> &'static Arc<Mutex<Mixer>> {
    static MIXER: OnceLock<Arc<Mutex<Mixer>>> = OnceLock::new();
    MIXER.get_or_init(|| Arc::new(Mutex::new(Mixer::default())))
}

fn with_mixer<R>(f: impl FnOnce(&mut Mixer) -> R) -> R {
    let mut guard = mixer().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

/// The shared renderer, created on first playback; null means "not created".
static RENDERER: AtomicPtr<sys::AudioRenderer> = AtomicPtr::new(std::ptr::null_mut());
/// Serializes renderer creation and release.
static RENDERER_LOCK: Mutex<()> = Mutex::new(());

fn stream_result(code: c_int, operation: &str) -> Result<()> {
    if code == sys::AUDIOSTREAM_SUCCESS {
        Ok(())
    } else {
        Err(anyhow!("{operation} failed with result {code}"))
    }
}

/// The renderer callback runs on a system audio thread. It must not block: the
/// mixer lock is only taken when free, otherwise the buffer is left silent.
extern "C" fn renderer_write_data(
    _renderer: *mut sys::AudioRenderer,
    _user_data: *mut std::os::raw::c_void,
    buffer: *mut std::os::raw::c_void,
    length: c_int,
) -> c_int {
    if buffer.is_null() || length <= 0 {
        return 0;
    }
    // SAFETY: `buffer` is `length` bytes of interleaved S16LE audio owned by the
    // renderer for the duration of this callback.
    unsafe {
        let output = std::slice::from_raw_parts_mut(buffer as *mut i16, length as usize / 2);
        // The lock only covers a walk over the queue; blocking here is cheaper
        // than dropping a frame, which would be audible as a click.
        let mut mixer = mixer()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mixer.fill(output);
    }
    0
}

static RENDERER_CALLBACKS: sys::AudioRendererCallbacks = sys::AudioRendererCallbacks {
    write_data: Some(renderer_write_data),
    stream_event: None,
    interrupt_event: None,
    error: None,
};

/// Creates and starts the shared renderer if it is not running yet.
fn ensure_renderer() -> Result<()> {
    if !RENDERER.load(Ordering::Acquire).is_null() {
        return Ok(());
    }
    let _guard = RENDERER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !RENDERER.load(Ordering::Acquire).is_null() {
        return Ok(());
    }

    let mut builder: *mut sys::AudioStreamBuilder = std::ptr::null_mut();
    // SAFETY: the builder is destroyed on every path, and the generated
    // renderer is stored in the slot above.
    unsafe {
        stream_result(
            sys::OH_AudioStreamBuilder_Create(&mut builder, sys::AUDIOSTREAM_TYPE_RENDERER),
            "creating the audio stream builder",
        )?;
        let setup = (|| -> Result<*mut sys::AudioRenderer> {
            stream_result(
                sys::OH_AudioStreamBuilder_SetSamplingRate(builder, SAMPLE_RATE.get() as c_int),
                "setting the renderer sample rate",
            )?;
            stream_result(
                sys::OH_AudioStreamBuilder_SetChannelCount(builder, CHANNEL_COUNT.get() as c_int),
                "setting the renderer channel count",
            )?;
            stream_result(
                sys::OH_AudioStreamBuilder_SetSampleFormat(
                    builder,
                    sys::AUDIOSTREAM_SAMPLE_S16LE,
                ),
                "setting the renderer sample format",
            )?;
            stream_result(
                sys::OH_AudioStreamBuilder_SetEncodingType(
                    builder,
                    sys::AUDIOSTREAM_ENCODING_TYPE_RAW,
                ),
                "setting the renderer encoding type",
            )?;
            stream_result(
                sys::OH_AudioStreamBuilder_SetRendererInfo(
                    builder,
                    sys::AUDIOSTREAM_USAGE_NOTIFICATION,
                ),
                "setting the renderer usage",
            )?;
            stream_result(
                sys::OH_AudioStreamBuilder_SetRendererCallback(
                    builder,
                    &RENDERER_CALLBACKS as *const sys::AudioRendererCallbacks
                        as *mut sys::AudioRendererCallbacks,
                ),
                "setting the renderer callback",
            )?;
            let mut renderer: *mut sys::AudioRenderer = std::ptr::null_mut();
            stream_result(
                sys::OH_AudioStreamBuilder_GenerateRenderer(builder, &mut renderer),
                "generating the audio renderer",
            )?;
            if renderer.is_null() {
                return Err(anyhow!("the audio renderer is null"));
            }
            if let Err(error) = stream_result(
                sys::OH_AudioRenderer_Start(renderer),
                "starting the audio renderer",
            ) {
                sys::OH_AudioRenderer_Release(renderer);
                return Err(error);
            }
            Ok(renderer)
        })();
        sys::OH_AudioStreamBuilder_Destroy(builder);
        match setup {
            Ok(renderer) => {
                RENDERER.store(renderer, Ordering::Release);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

fn release_renderer() {
    let _guard = RENDERER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let renderer = RENDERER.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if renderer.is_null() {
        return;
    }
    // SAFETY: the pointer came from `ensure_renderer` and is released exactly
    // once because it was swapped out of the atomic above.
    unsafe {
        sys::OH_AudioRenderer_Stop(renderer);
        sys::OH_AudioRenderer_Release(renderer);
    }
}

/// Incremented on every queued clip, so the idle release thread can tell that
/// new audio arrived while it was sleeping.
static PLAY_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Whether an idle release thread is already running.
static IDLE_RELEASE_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Closes the renderer once the mixer has been idle for a while. The renderer
/// otherwise keeps the output device open (and drains power) long after the
/// last notification sound finished.
fn schedule_idle_release() {
    if IDLE_RELEASE_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("audio-idle-release".to_string())
        .spawn(|| loop {
            std::thread::sleep(IDLE_RELEASE_DELAY);
            let generation = PLAY_GENERATION.load(Ordering::Acquire);
            if !with_mixer(|mixer| mixer.is_empty()) {
                continue;
            }
            // Confirm after another delay so a clip queued while this thread
            // was waking up is not cut off.
            std::thread::sleep(IDLE_RELEASE_DELAY);
            if PLAY_GENERATION.load(Ordering::Acquire) != generation {
                continue;
            }
            if !with_mixer(|mixer| mixer.is_empty()) {
                continue;
            }
            release_renderer();
            IDLE_RELEASE_SCHEDULED.store(false, Ordering::Release);
            return;
        });
    if let Err(error) = spawned {
        IDLE_RELEASE_SCHEDULED.store(false, Ordering::Release);
        log::warn!("audio_pipeline_ohos: cannot start the idle release thread: {error}");
    }
}

/// Plays a short bell tone through the same renderer used for notifications.
/// The platform layer calls this for terminal BEL events.
pub fn play_system_bell() {
    if let Err(error) = ensure_renderer() {
        log::warn!("audio_pipeline_ohos: cannot start the audio renderer for the bell: {error}");
        return;
    }
    with_mixer(|mixer| mixer.push(Arc::new(render_bell_tone())));
    PLAY_GENERATION.fetch_add(1, Ordering::AcqRel);
    schedule_idle_release();
}

/// Renders a short bell modelled after the macOS default alert sound (`Tink`,
/// shown as "Boop" in recent releases): a bright tap built from inharmonic
/// partials with a fast exponential decay, so it reads as a light "tink"
/// rather than a steady beep.
fn render_bell_tone() -> Vec<i16> {
    // The macOS alert concentrates its energy in the 1-4 kHz range, where
    // human hearing is most sensitive.
    const FUNDAMENTAL_HZ: f32 = 1900.0;
    // Inharmonic partials (frequency ratio, relative amplitude). A struck
    // object does not produce a harmonic series, and that is what gives the
    // sound its metallic character.
    const PARTIALS: [(f32, f32); 3] = [(1.0, 0.62), (2.42, 0.28), (3.76, 0.10)];
    const DURATION_MS: u32 = 160;
    const ATTACK_MS: u32 = 2;
    const TAIL_FADE_MS: u32 = 5;
    // Time constant of the exponential decay.
    const DECAY_TAU_MS: f32 = 38.0;
    const AMPLITUDE: f32 = 0.4;

    let sample_rate = SAMPLE_RATE.get() as f32;
    let frames = (sample_rate * DURATION_MS as f32 / 1000.0) as usize;
    let attack_frames = (sample_rate * ATTACK_MS as f32 / 1000.0) as usize;
    let fade_frames = (sample_rate * TAIL_FADE_MS as f32 / 1000.0) as usize;
    let decay_tau_seconds = DECAY_TAU_MS / 1000.0;
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let time = frame as f32 / sample_rate;
        let attack = if frame < attack_frames {
            frame as f32 / attack_frames.max(1) as f32
        } else {
            1.0
        };
        let tail_fade = if frame + fade_frames >= frames {
            (frames - frame) as f32 / fade_frames.max(1) as f32
        } else {
            1.0
        };
        let decay = (-time / decay_tau_seconds).exp();
        let mut value = 0.0;
        for (ratio, amplitude) in PARTIALS {
            let phase = time * FUNDAMENTAL_HZ * ratio * std::f32::consts::TAU;
            value += phase.sin() * amplitude;
        }
        let sample = (value * attack * tail_fade * decay * AMPLITUDE * i16::MAX as f32) as i16;
        samples.push(sample);
        samples.push(sample);
    }
    samples
}

impl Audio {
    pub fn play_sound(sound: Sound, cx: &mut App) {
        let Some(samples) = load_sound(sound, cx) else {
            return;
        };
        if let Err(error) = ensure_renderer() {
            log::warn!("audio_pipeline_ohos: cannot start the audio renderer: {error}");
            return;
        }
        with_mixer(|mixer| mixer.push(samples));
        PLAY_GENERATION.fetch_add(1, Ordering::AcqRel);
        schedule_idle_release();
    }

    pub fn end_call(_cx: &mut App) {
        with_mixer(|mixer| mixer.clear());
        release_renderer();
    }
}

pub fn init(_cx: &mut App) {
    // The renderer is created lazily on first playback, so nothing to set up here.
}

/// Asks the host ArkTS side for the microphone permission. Must be called on
/// the main thread; the returned future may be awaited on any thread.
pub fn request_microphone_permission() -> Pin<Box<dyn Future<Output = Result<bool>> + Send>> {
    use openharmony_ability_plugin_permission::PermissionExt as _;

    let pending = openharmony_ability::global_app()
        .map(|app| app.request_permission("ohos.permission.MICROPHONE"));
    Box::pin(async move {
        let Some(pending) = pending else {
            return Err(anyhow!("the OpenHarmony app is not available"));
        };
        let codes = pending
            .await
            .map_err(|error| anyhow!("requesting the microphone permission failed: {error}"))?;
        Ok(codes.iter().all(|code| code.code == 0))
    })
}

pub fn ensure_devices_initialized(cx: &mut App) {
    if !cx.has_global::<AvailableAudioDevices>() {
        cx.set_global(AvailableAudioDevices(enumerate_devices()));
    }
}

pub fn resolve_device(device_id: Option<&DeviceId>, _input: bool) -> Result<Device> {
    Ok(Device {
        id: device_id.cloned(),
    })
}

pub fn open_input_stream(device_id: Option<DeviceId>) -> Result<InputStream> {
    InputStream::open(device_id)
}

pub fn open_test_output(device_id: Option<DeviceId>) -> Result<TestOutput> {
    ensure_renderer().context("opening the audio output stream")?;
    let renderer = RENDERER.load(Ordering::Acquire);
    if !renderer.is_null() {
        select_output_device(renderer, device_id.as_ref());
    }
    // A test that produces no audio still opened the device; close it again
    // unless the test stream starts feeding the mixer.
    schedule_idle_release();
    Ok(TestOutput)
}

fn sound_cache() -> &'static Mutex<HashMap<Sound, Arc<Vec<i16>>>> {
    static SOUND_CACHE: OnceLock<Mutex<HashMap<Sound, Arc<Vec<i16>>>>> = OnceLock::new();
    SOUND_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_sound(sound: Sound, cx: &mut App) -> Option<Arc<Vec<i16>>> {
    if let Some(cached) = sound_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&sound)
        .cloned()
    {
        return Some(cached);
    }
    let asset_path = format!("sounds/{}.wav", sound.file());
    let bytes = cx
        .asset_source()
        .load(&asset_path)
        .map_err(|error| log::warn!("audio_pipeline_ohos: cannot read {asset_path}: {error}"))
        .ok()
        .flatten()?;
    let samples = match decode_wav(&bytes) {
        Ok(samples) => Arc::new(samples),
        Err(error) => {
            log::warn!("audio_pipeline_ohos: cannot decode {asset_path}: {error}");
            return None;
        }
    };
    sound_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(sound, samples.clone());
    Some(samples)
}

/// Decodes a PCM WAV file into interleaved stereo S16LE at [`SAMPLE_RATE`].
fn decode_wav(bytes: &[u8]) -> Result<Vec<i16>> {
    let (channels, sample_rate, bits, data) = parse_wav(bytes)?;
    let mono = decode_pcm(data, bits, channels)?;
    let resampled = resample(&mono, sample_rate, SAMPLE_RATE.get());
    let mut stereo = Vec::with_capacity(resampled.len() * 2);
    for sample in resampled {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        stereo.push(value);
        stereo.push(value);
    }
    Ok(stereo)
}

fn parse_wav(bytes: &[u8]) -> Result<(u16, u32, u16, &[u8])> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(anyhow!("not a WAV file"));
    }
    let mut offset = 12;
    let mut format = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let body_start = offset + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        if id == b"fmt " {
            if body.len() < 16 {
                return Err(anyhow!("truncated WAV format chunk"));
            }
            let audio_format = u16::from_le_bytes(body[0..2].try_into()?);
            let channels = u16::from_le_bytes(body[2..4].try_into()?);
            let sample_rate = u32::from_le_bytes(body[4..8].try_into()?);
            let bits = u16::from_le_bytes(body[14..16].try_into()?);
            if audio_format != 1 {
                return Err(anyhow!("unsupported WAV encoding {audio_format}"));
            }
            format = Some((channels, sample_rate, bits));
        } else if id == b"data" {
            let (channels, sample_rate, bits) =
                format.ok_or_else(|| anyhow!("WAV data before format"))?;
            return Ok((channels, sample_rate, bits, body));
        }
        offset = body_start + size + (size & 1);
    }
    Err(anyhow!("WAV file has no data chunk"))
}

fn decode_pcm(data: &[u8], bits: u16, channels: u16) -> Result<Vec<f32>> {
    if channels == 0 {
        return Err(anyhow!("WAV file has no channels"));
    }
    let mut samples = Vec::new();
    match bits {
        16 => {
            for frame in data.chunks_exact(2 * channels as usize) {
                let mut sum = 0.0;
                for channel in 0..channels as usize {
                    let offset = channel * 2;
                    let value = i16::from_le_bytes([frame[offset], frame[offset + 1]]);
                    sum += value as f32 / i16::MAX as f32;
                }
                samples.push(sum / channels as f32);
            }
        }
        24 => {
            let stride = 3 * channels as usize;
            for frame in data.chunks_exact(stride) {
                let mut sum = 0.0;
                for channel in 0..channels as usize {
                    let offset = channel * 3;
                    let value = i32::from_le_bytes([
                        frame[offset],
                        frame[offset + 1],
                        frame[offset + 2],
                        if frame[offset + 2] & 0x80 != 0 {
                            0xFF
                        } else {
                            0
                        },
                    ]);
                    sum += value as f32 / (i32::MAX >> 8) as f32;
                }
                samples.push(sum / channels as f32);
            }
        }
        other => return Err(anyhow!("unsupported WAV bit depth {other}")),
    }
    Ok(samples)
}

fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let length = (samples.len() as f64 * ratio).ceil() as usize;
    (0..length)
        .map(|index| {
            let position = index as f64 / ratio;
            let left = position.floor() as usize;
            let fraction = position - left as f64;
            let a = samples[left.min(samples.len() - 1)];
            let b = samples[(left + 1).min(samples.len() - 1)];
            a + (b - a) * fraction as f32
        })
        .collect()
}

fn enumerate_devices() -> Vec<AudioDeviceInfo> {
    let mut devices = Vec::new();
    // SAFETY: every pointer is checked for null before use, and the descriptor
    // array is released through the same routing manager that produced it.
    unsafe {
        let mut manager: *mut sys::AudioRoutingManager = std::ptr::null_mut();
        if sys::OH_AudioManager_GetAudioRoutingManager(&mut manager)
            != sys::AUDIOCOMMON_RESULT_SUCCESS
            || manager.is_null()
        {
            return devices;
        }
        let mut array: *mut sys::AudioDeviceDescriptorArray = std::ptr::null_mut();
        if sys::OH_AudioRoutingManager_GetDevices(
            manager,
            sys::AUDIO_DEVICE_FLAG_ALL,
            &mut array,
        ) != sys::AUDIOCOMMON_RESULT_SUCCESS
            || array.is_null()
        {
            return devices;
        }
        let descriptors = &*array;
        for index in 0..descriptors.size as usize {
            let descriptor = *descriptors.descriptors.add(index);
            if descriptor.is_null() {
                continue;
            }
            let mut device_type: c_int = 0;
            let mut id: u32 = 0;
            let mut name: *mut c_char = std::ptr::null_mut();
            let mut display_name: *mut c_char = std::ptr::null_mut();
            if sys::OH_AudioDeviceDescriptor_GetDeviceType(descriptor, &mut device_type)
                != sys::AUDIOCOMMON_RESULT_SUCCESS
            {
                continue;
            }
            sys::OH_AudioDeviceDescriptor_GetDeviceId(descriptor, &mut id);
            sys::OH_AudioDeviceDescriptor_GetDeviceName(descriptor, &mut name);
            sys::OH_AudioDeviceDescriptor_GetDeviceDisplayName(descriptor, &mut display_name);
            let raw_name = if name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(name).to_string_lossy().into_owned()
            };
            let raw_display_name = if display_name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(display_name).to_string_lossy().into_owned()
            };
            // Some builds report empty strings; fall back to the device type so
            // the settings dropdown never contains a blank row.
            let name = if !raw_display_name.is_empty() {
                raw_display_name.clone()
            } else if !raw_name.is_empty() {
                raw_name.clone()
            } else {
                sys::device_type_label(device_type)
            };
            let (input, output) = sys::device_type_capabilities(device_type);
            if !input && !output {
                continue;
            }
            devices.push(AudioDeviceInfo {
                id: id.to_string(),
                desc: DeviceDescription {
                    name,
                    input,
                    output,
                },
            });
        }
        sys::OH_AudioRoutingManager_ReleaseDevices(manager, array);
    }
    devices
}

/// Calls `f` with the device descriptor whose id matches `device_id`. The
/// descriptor array is released before this function returns.
fn with_device_descriptor<R>(
    device_id: u32,
    f: impl FnOnce(*mut sys::AudioDeviceDescriptor) -> R,
) -> Option<R> {
    // SAFETY: the descriptor array is released before returning, and `f` only
    // uses the pointer while the array is alive.
    unsafe {
        let mut routing_manager: *mut sys::AudioRoutingManager = std::ptr::null_mut();
        if sys::OH_AudioManager_GetAudioRoutingManager(&mut routing_manager)
            != sys::AUDIOCOMMON_RESULT_SUCCESS
            || routing_manager.is_null()
        {
            return None;
        }
        let mut array: *mut sys::AudioDeviceDescriptorArray = std::ptr::null_mut();
        if sys::OH_AudioRoutingManager_GetDevices(
            routing_manager,
            sys::AUDIO_DEVICE_FLAG_ALL,
            &mut array,
        ) != sys::AUDIOCOMMON_RESULT_SUCCESS
            || array.is_null()
        {
            return None;
        }
        let descriptors = &*array;
        let mut result = None;
        for index in 0..descriptors.size as usize {
            let descriptor = *descriptors.descriptors.add(index);
            if descriptor.is_null() {
                continue;
            }
            let mut id: u32 = 0;
            if sys::OH_AudioDeviceDescriptor_GetDeviceId(descriptor, &mut id)
                != sys::AUDIOCOMMON_RESULT_SUCCESS
                || id != device_id
            {
                continue;
            }
            result = Some(f(descriptor));
            break;
        }
        sys::OH_AudioRoutingManager_ReleaseDevices(routing_manager, array);
        result
    }
}

/// Returns the enhanced-routing manager when the device supports selecting an
/// output or input device per stream.
fn enhanced_routing_manager() -> Option<*mut sys::AudioDeviceEnhanceManager> {
    // SAFETY: the manager is owned by the audio service; only its pointer is
    // kept for the duration of a single selection call.
    unsafe {
        let mut manager: *mut sys::AudioDeviceEnhanceManager = std::ptr::null_mut();
        if sys::OH_AudioManager_GetAudioDeviceEnhanceManager(&mut manager)
            != sys::AUDIOCOMMON_RESULT_SUCCESS
            || manager.is_null()
        {
            return None;
        }
        let mut supported = false;
        if sys::OH_AudioDeviceEnhanceManager_IsEnhancedRoutingSupported(manager, &mut supported)
            != sys::AUDIOCOMMON_RESULT_SUCCESS
            || !supported
        {
            return None;
        }
        Some(manager)
    }
}

fn device_id_number(device_id: &DeviceId) -> Option<u32> {
    match device_id.parse::<u32>() {
        Ok(id) => Some(id),
        Err(error) => {
            log::warn!("audio_pipeline_ohos: invalid device id {device_id:?}: {error}");
            None
        }
    }
}

fn select_output_device(renderer: *mut sys::AudioRenderer, device_id: Option<&DeviceId>) {
    let Some(device_id) = device_id.and_then(device_id_number) else {
        return;
    };
    let Some(manager) = enhanced_routing_manager() else {
        log::warn!(
            "audio_pipeline_ohos: enhanced routing is unavailable; using the system default output"
        );
        return;
    };
    let result = with_device_descriptor(device_id, |descriptor| {
        // SAFETY: `renderer` is the live renderer from `ensure_renderer`;
        // `manager` and `descriptor` are valid for this call.
        unsafe {
            sys::OH_AudioDeviceEnhanceManager_SelectOutputDeviceForAudioRenderer(
                manager, renderer, descriptor,
            )
        }
    });
    match result {
        Some(code) if code == sys::AUDIOCOMMON_RESULT_SUCCESS => {}
        Some(code) => log::warn!(
            "audio_pipeline_ohos: selecting output device {device_id} failed with result {code}"
        ),
        None => log::warn!("audio_pipeline_ohos: output device {device_id} is not available"),
    }
}

fn select_input_device(capturer: *mut sys::AudioCapturer, device_id: Option<&DeviceId>) {
    let Some(device_id) = device_id.and_then(device_id_number) else {
        return;
    };
    let Some(manager) = enhanced_routing_manager() else {
        log::warn!(
            "audio_pipeline_ohos: enhanced routing is unavailable; using the system default input"
        );
        return;
    };
    let result = with_device_descriptor(device_id, |descriptor| {
        // SAFETY: `capturer` is the live capturer from `InputStream::open`;
        // `manager` and `descriptor` are valid for this call.
        unsafe {
            sys::OH_AudioDeviceEnhanceManager_SelectInputDeviceForAudioCapturer(
                manager, capturer, descriptor,
            )
        }
    });
    match result {
        Some(code) if code == sys::AUDIOCOMMON_RESULT_SUCCESS => {}
        Some(code) => log::warn!(
            "audio_pipeline_ohos: selecting input device {device_id} failed with result {code}"
        ),
        None => log::warn!("audio_pipeline_ohos: input device {device_id} is not available"),
    }
}

/// Queued capture samples shared with the capturer callback.
struct CaptureQueue {
    state: Mutex<CaptureState>,
    ready: std::sync::Condvar,
}

struct CaptureState {
    samples: VecDeque<i16>,
    /// Set when the owning `InputStream` is dropped, so a consumer waiting for
    /// samples can stop instead of waiting forever.
    closed: bool,
}

fn capture_queue() -> &'static Arc<CaptureQueue> {
    static QUEUE: OnceLock<Arc<CaptureQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        Arc::new(CaptureQueue {
            state: Mutex::new(CaptureState {
                samples: VecDeque::new(),
                closed: false,
            }),
            ready: std::sync::Condvar::new(),
        })
    })
}

extern "C" fn capturer_read_data(
    _capturer: *mut sys::AudioCapturer,
    _user_data: *mut std::os::raw::c_void,
    buffer: *mut std::os::raw::c_void,
    length: c_int,
) -> c_int {
    if buffer.is_null() || length <= 0 {
        return 0;
    }
    // SAFETY: `buffer` holds `length` bytes of S16LE audio owned by the
    // capturer for the duration of this callback.
    unsafe {
        let samples = std::slice::from_raw_parts(buffer as *const i16, length as usize / 2);
        if let Ok(mut state) = capture_queue().state.lock() {
            state.samples.extend(samples.iter().copied());
            while state.samples.len() > MAX_CAPTURED_SAMPLES {
                state.samples.pop_front();
            }
        }
        capture_queue().ready.notify_all();
    }
    0
}

static CAPTURER_CALLBACKS: sys::AudioCapturerCallbacks = sys::AudioCapturerCallbacks {
    read_data: Some(capturer_read_data),
    stream_event: None,
    interrupt_event: None,
    error: None,
};

pub struct InputStream {
    capturer: *mut sys::AudioCapturer,
}

// SAFETY: the capturer pointer is only touched by `Drop` on the owning thread,
// and captured audio travels through the mutex-protected queue.
unsafe impl Send for InputStream {}

impl InputStream {
    fn open(device_id: Option<DeviceId>) -> Result<Self> {
        {
            let queue = capture_queue();
            let mut state = queue
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.samples.clear();
            state.closed = false;
        }
        let mut builder: *mut sys::AudioStreamBuilder = std::ptr::null_mut();
        // SAFETY: the builder is destroyed on every path, and the capturer is
        // released in `Drop`.
        unsafe {
            stream_result(
                sys::OH_AudioStreamBuilder_Create(&mut builder, sys::AUDIOSTREAM_TYPE_CAPTURER),
                "creating the capture stream builder",
            )?;
            let setup = (|| -> Result<*mut sys::AudioCapturer> {
                stream_result(
                    sys::OH_AudioStreamBuilder_SetSamplingRate(builder, SAMPLE_RATE.get() as c_int),
                    "setting the capture sample rate",
                )?;
                stream_result(
                    sys::OH_AudioStreamBuilder_SetChannelCount(
                        builder,
                        CHANNEL_COUNT.get() as c_int,
                    ),
                    "setting the capture channel count",
                )?;
                stream_result(
                    sys::OH_AudioStreamBuilder_SetSampleFormat(
                        builder,
                        sys::AUDIOSTREAM_SAMPLE_S16LE,
                    ),
                    "setting the capture sample format",
                )?;
                stream_result(
                    sys::OH_AudioStreamBuilder_SetEncodingType(
                        builder,
                        sys::AUDIOSTREAM_ENCODING_TYPE_RAW,
                    ),
                    "setting the capture encoding type",
                )?;
                stream_result(
                    sys::OH_AudioStreamBuilder_SetCapturerInfo(
                        builder,
                        sys::AUDIOSTREAM_SOURCE_TYPE_MIC,
                    ),
                    "setting the capture source",
                )?;
                stream_result(
                    sys::OH_AudioStreamBuilder_SetCapturerCallback(
                        builder,
                        &CAPTURER_CALLBACKS as *const sys::AudioCapturerCallbacks
                            as *mut sys::AudioCapturerCallbacks,
                    ),
                    "setting the capture callback",
                )?;
                let mut capturer: *mut sys::AudioCapturer = std::ptr::null_mut();
                stream_result(
                    sys::OH_AudioStreamBuilder_GenerateCapturer(builder, &mut capturer),
                    "generating the audio capturer",
                )?;
                if capturer.is_null() {
                    return Err(anyhow!("the audio capturer is null"));
                }
                select_input_device(capturer, device_id.as_ref());
                if let Err(error) = stream_result(
                    sys::OH_AudioCapturer_Start(capturer),
                    "starting the audio capturer",
                ) {
                    sys::OH_AudioCapturer_Release(capturer);
                    return Err(error);
                }
                Ok(capturer)
            })();
            sys::OH_AudioStreamBuilder_Destroy(builder);
            match setup {
                Ok(capturer) => Ok(Self { capturer }),
                Err(error) => Err(error).context("opening the audio input stream"),
            }
        }
    }
}

impl Iterator for InputStream {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let queue = capture_queue();
        let mut state = queue
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(sample) = state.samples.pop_front() {
                return Some(sample as f32 / i16::MAX as f32);
            }
            if state.closed {
                return None;
            }
            // Wait for the capturer callback instead of spinning, but with a
            // timeout: the consumer also drives the stop check in
            // `periodic_access`, so an infinite wait would keep the capture
            // stream alive after the test stops.
            let (next_state, wait_result) = queue
                .ready
                .wait_timeout(state, CAPTURE_QUEUE_WAIT)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next_state;
            if wait_result.timed_out() {
                // Nothing captured yet; emit silence so the source keeps
                // producing samples and the stop signal is observed.
                return Some(0.0);
            }
        }
    }
}

impl Source for InputStream {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        CHANNEL_COUNT
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Drop for InputStream {
    fn drop(&mut self) {
        {
            let queue = capture_queue();
            let mut state = queue
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.closed = true;
        }
        capture_queue().ready.notify_all();
        if self.capturer.is_null() {
            return;
        }
        // SAFETY: the capturer was created by `InputStream::open` and is only
        // released here.
        unsafe {
            sys::OH_AudioCapturer_Stop(self.capturer);
            sys::OH_AudioCapturer_Release(self.capturer);
        }
    }
}

pub struct TestOutput;

impl TestOutput {
    pub fn mixer(&self) -> TestMixer {
        TestMixer
    }
}

pub struct TestMixer;

impl TestMixer {
    pub fn add<S>(&self, source: S)
    where
        S: Source<Item = Sample> + Send + 'static,
    {
        std::thread::spawn(move || {
            let mut chunk = Vec::with_capacity(CHUNK_SAMPLES);
            for sample in source {
                chunk.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                if chunk.len() >= CHUNK_SAMPLES {
                    let samples = Arc::new(std::mem::take(&mut chunk));
                    with_mixer(|mixer| mixer.push(samples));
                    PLAY_GENERATION.fetch_add(1, Ordering::AcqRel);
                }
            }
            if !chunk.is_empty() {
                with_mixer(|mixer| mixer.push(Arc::new(chunk)));
                PLAY_GENERATION.fetch_add(1, Ordering::AcqRel);
            }
            // The test stream ended: let the renderer close once the queued
            // audio has played out.
            schedule_idle_release();
        });
    }
}

pub struct Device {
    #[allow(dead_code)]
    id: Option<DeviceId>,
}

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo {
    pub id: DeviceId,
    pub desc: DeviceDescription,
}

impl AudioDeviceInfo {
    pub fn matches_input(&self, is_input: bool) -> bool {
        if is_input {
            self.desc.supports_input()
        } else {
            self.desc.supports_output()
        }
    }

    pub fn matches(&self, id: &DeviceId, is_input: bool) -> bool {
        &self.id == id && self.matches_input(is_input)
    }
}

impl std::fmt::Display for AudioDeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.desc.name.fmt(f)
    }
}

#[derive(Default, Clone, Debug)]
pub struct AvailableAudioDevices(pub Vec<AudioDeviceInfo>);

impl Global for AvailableAudioDevices {}

#[derive(Clone, Debug)]
pub struct DeviceDescription {
    pub name: String,
    pub input: bool,
    pub output: bool,
}

impl DeviceDescription {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn supports_input(&self) -> bool {
        self.input
    }

    pub fn supports_output(&self) -> bool {
        self.output
    }
}
