// ===== [OHOS PORT] file added by the OHOS port =====
// OHOS has neither a `cpal` backend nor a `libwebrtc` build, so this module is
// compiled instead of `audio_pipeline` on `target_env = "ohos"` (see the module
// selection in `crates/audio/src/audio.rs`). It keeps the same public surface as
// `audio_pipeline` so that the rest of the workspace compiles unchanged; audio
// device enumeration, capture and playback are not wired up to the OHOS audio
// APIs (ohaudio) yet and report an error or a warning when used.

use anyhow::{anyhow, Result};
use gpui::{App, Global};

use crate::DeviceId;
use crate::Sound;

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
pub struct Audio;

impl Global for Audio {}

impl Audio {
    pub fn play_sound(sound: Sound, _cx: &mut App) {
        log::warn!(
            "audio_pipeline_ohos: play_sound({sound:?}) is not implemented on OHOS yet, no sound will be played"
        );
    }

    pub fn end_call(_cx: &mut App) {
        log::warn!("audio_pipeline_ohos: end_call is not implemented on OHOS yet");
    }
}

pub fn init(_cx: &mut App) {
    log::info!("audio_pipeline_ohos: init, OHOS audio device management is not wired up yet");
}

pub fn ensure_devices_initialized(cx: &mut App) {
    if !cx.has_global::<AvailableAudioDevices>() {
        log::info!("audio_pipeline_ohos: ensure_devices_initialized, publishing an empty device list");
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
