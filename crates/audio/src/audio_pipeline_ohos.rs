// ===== [OHOS PORT] file added by the OHOS port =====
// OHOS has neither a `cpal` backend nor a `libwebrtc` build, so this module is
// compiled instead of `audio_pipeline` on `target_env = "ohos"` (see the module
// selection in `crates/audio/src/audio.rs`). It keeps the same public surface as
// `audio_pipeline` so that the rest of the workspace compiles unchanged.
//
// Short sound effects are played through the on-device SoundPool bridge plugin.
// Audio device enumeration and capture (used by calls and the audio test panel)
// are still not wired up to the OHOS audio APIs and report an error when used.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use gpui::{App, AsyncApp, BorrowAppContext, Global};
use openharmony_ability::global_app;
use openharmony_ability_plugin_play_notification::{PlayNotificationExt, SoundPlayParameters, SoundSource};

use crate::DeviceId;
use crate::Sound;

/// Simultaneous streams the pool may mix.
const MAX_STREAMS: u32 = 4;
/// `audio.StreamUsage.STREAM_USAGE_NOTIFICATION`, used for short notifications.
const STREAM_USAGE_NOTIFICATION: i32 = 7;
/// Default renderer flags for the notification renderer.
const RENDERER_FLAGS: i32 = 0;

/// The methods `settings_ui::pages::audio_test_window` relies on to adapt a raw
/// microphone stream. On OHOS no stream is ever produced, so they are identity
/// operations that exist only to keep the call site compiling.
pub trait RodioExt: rodio::Source + Sized {
    fn constant_params(
        self,
        _channel_count: rodio::ChannelCount,
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

#[derive(Default)]
pub struct Audio {
    /// Whether the SoundPool bridge plugin created its pool.
    pool_ready: bool,
    /// Loaded sounds, keyed by the effect they belong to.
    sound_ids: HashMap<Sound, u32>,
}

/// Serializes first-use cache writes: two concurrent first plays of the same
/// sound must not write the same file at once.
static SOUND_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Returns the cache path for one sound effect, writing it from the embedded
/// asset on first use.
///
/// Zed embeds `sounds/**/*` into the binary (see `crates/assets`), but SoundPool
/// can only load a file, so the bytes are materialized once under the custom
/// data directory.
fn ensure_sound_file(sound: Sound, cx: &mut AsyncApp) -> Option<PathBuf> {
    let path = paths::data_dir()
        .join("cache")
        .join("sounds")
        .join(format!("{}.wav", sound.file()));

    // The lock covers the check and the write. It is a synchronous lock held
    // across file I/O only; this function never awaits while holding it.
    let _lock = SOUND_CACHE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // An interrupted previous write can leave an empty file behind; only a
    // non-empty file is a usable cache entry.
    if path
        .metadata()
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
    {
        return Some(path);
    }

    let asset_path = format!("sounds/{}.wav", sound.file());
    let bytes = cx.update(|cx| match cx.asset_source().load(&asset_path) {
        Ok(Some(bytes)) => Some(bytes.into_owned()),
        Ok(None) => {
            log::warn!("audio_pipeline_ohos: no embedded asset at {asset_path}");
            None
        }
        Err(error) => {
            log::warn!("audio_pipeline_ohos: cannot read embedded asset {asset_path}: {error}");
            None
        }
    });
    let Some(bytes) = bytes else {
        return None;
    };

    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("audio_pipeline_ohos: cannot create sound cache directory: {error}");
            return None;
        }
    }

    // Write a sibling temporary file and rename it into place, so an
    // interrupted write cannot leave a truncated file that later loads fail on.
    let temporary_path = path.with_extension("wav.tmp");
    if let Err(error) = std::fs::write(&temporary_path, bytes) {
        log::warn!(
            "audio_pipeline_ohos: cannot write temporary sound cache {}: {error}",
            temporary_path.display()
        );
        return None;
    }
    if let Err(error) = std::fs::rename(&temporary_path, &path) {
        log::warn!(
            "audio_pipeline_ohos: cannot move temporary sound cache into {}: {error}",
            path.display()
        );
        if let Err(remove_error) = std::fs::remove_file(&temporary_path) {
            log::warn!(
                "audio_pipeline_ohos: cannot remove temporary sound cache {}: {remove_error}",
                temporary_path.display()
            );
        }
        return None;
    }
    Some(path)
}

impl Global for Audio {}

impl Audio {
    pub fn play_sound(sound: Sound, cx: &mut App) {
        cx.spawn(async move |cx| {
            let Some(app) = global_app() else {
                log::warn!(
                    "audio_pipeline_ohos: no OpenHarmony app, cannot play {sound:?}"
                );
                return;
            };

            let pool_ready = cx.update(|cx| {
                cx.update_default_global(|audio: &mut Audio, _cx| audio.pool_ready)
            });
            if !pool_ready {
                if let Err(error) = app
                    .create_sound_pool(MAX_STREAMS, STREAM_USAGE_NOTIFICATION, RENDERER_FLAGS)
                    .await
                {
                    log::warn!("audio_pipeline_ohos: cannot create SoundPool: {error}");
                    return;
                }
                cx.update(|cx| {
                    cx.update_default_global(|audio: &mut Audio, _cx| audio.pool_ready = true)
                });
            }

            let cached_sound_id = cx.update(|cx| {
                cx.update_default_global(|audio: &mut Audio, _cx| {
                    audio.sound_ids.get(&sound).copied()
                })
            });
            let sound_id = match cached_sound_id {
                Some(sound_id) => sound_id,
                None => {
                    let Some(path) = ensure_sound_file(sound, cx) else {
                        return;
                    };
                    let source = SoundSource::File(path.to_string_lossy().into_owned());
                    match app.load_sound(source).await {
                        Ok(sound_id) => {
                            cx.update(|cx| {
                                cx.update_default_global(|audio: &mut Audio, _cx| {
                                    audio.sound_ids.insert(sound, sound_id);
                                });
                            });
                            sound_id
                        }
                        Err(error) => {
                            log::warn!(
                                "audio_pipeline_ohos: cannot load sound {sound:?}: {error}"
                            );
                            // A failed load usually means the cached file is
                            // unusable; drop it so the next play rewrites it
                            // from the embedded asset.
                            if let Err(remove_error) = std::fs::remove_file(&path) {
                                log::warn!(
                                    "audio_pipeline_ohos: cannot remove cached sound {}: {remove_error}",
                                    path.display()
                                );
                            }
                            return;
                        }
                    }
                }
            };

            if let Err(error) = app
                .play_sound(sound_id, SoundPlayParameters::default())
                .await
            {
                log::warn!("audio_pipeline_ohos: cannot play sound {sound:?}: {error}");
            }
        })
        .detach();
    }

    pub fn end_call(cx: &mut App) {
        cx.update_default_global(|audio: &mut Audio, _cx| {
            audio.pool_ready = false;
            audio.sound_ids.clear();
        });
        cx.spawn(async move |_cx| {
            let Some(app) = global_app() else {
                return;
            };
            if let Err(error) = app.release_sound_pool().await {
                log::warn!("audio_pipeline_ohos: cannot release SoundPool: {error}");
            }
        })
        .detach();
    }
}

pub fn init(_cx: &mut App) {
    // The sound pool is created lazily on first playback, so nothing to set up here.
}

pub fn ensure_devices_initialized(cx: &mut App) {
    if !cx.has_global::<AvailableAudioDevices>() {
        cx.set_global(AvailableAudioDevices(Vec::new()));
    }
}

pub fn resolve_device(_device_id: Option<&DeviceId>, _input: bool) -> Result<Device> {
    log::warn!("audio_pipeline_ohos: resolve_device, audio devices are not available on OHOS yet");
    Err(anyhow!("audio devices are not available on OHOS yet"))
}

pub fn open_input_stream(_device_id: Option<DeviceId>) -> Result<InputStream> {
    log::warn!("audio_pipeline_ohos: open_input_stream, audio input is not available on OHOS yet");
    Err(anyhow!("audio input is not available on OHOS yet"))
}

pub fn open_test_output(_device_id: Option<DeviceId>) -> Result<TestOutput> {
    log::warn!("audio_pipeline_ohos: open_test_output, audio output is not available on OHOS yet");
    Err(anyhow!("audio output is not available on OHOS yet"))
}

pub struct Device;

pub struct InputStream;

impl Iterator for InputStream {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

impl rodio::Source for InputStream {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        crate::CHANNEL_COUNT
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        crate::SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
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
    pub fn add<S>(&self, _source: S) {}
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
