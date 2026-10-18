// ===== [OHOS PORT] file added by the OHOS port =====
// Minimal FFI declarations for the OpenHarmony audio NDK (`libohaudio.so`).
//
// Only the surface the audio pipeline needs is declared here: renderer and
// capturer construction, the renderer/capturer data callbacks, and the routing
// manager used to enumerate devices. The declarations mirror
// `native_audiostream_base.h`, `native_audiostreambuilder.h`,
// `native_audiorenderer.h`, `native_audiocapturer.h`,
// `native_audio_routing_manager.h` and `native_audio_device_base.h` from the
// OHOS SDK (API 12+).

use std::os::raw::{c_char, c_int, c_void};

pub type AudioStreamBuilder = c_void;
pub type AudioRenderer = c_void;
pub type AudioCapturer = c_void;
pub type AudioRoutingManager = c_void;
pub type AudioDeviceEnhanceManager = c_void;
pub type AudioDeviceDescriptor = c_void;

/// `OH_AudioStream_Type`
pub const AUDIOSTREAM_TYPE_RENDERER: c_int = 1;
pub const AUDIOSTREAM_TYPE_CAPTURER: c_int = 2;

/// `OH_AudioStream_SampleFormat`
pub const AUDIOSTREAM_SAMPLE_S16LE: c_int = 1;

/// `OH_AudioStream_EncodingType`
pub const AUDIOSTREAM_ENCODING_TYPE_RAW: c_int = 0;

/// `OH_AudioStream_Usage`
pub const AUDIOSTREAM_USAGE_NOTIFICATION: c_int = 7;
pub const AUDIOSTREAM_USAGE_MUSIC: c_int = 1;

/// `OH_AudioStream_SourceType`
pub const AUDIOSTREAM_SOURCE_TYPE_MIC: c_int = 0;

/// `OH_AudioDevice_Flag`
pub const AUDIO_DEVICE_FLAG_OUTPUT: c_int = 1;
pub const AUDIO_DEVICE_FLAG_INPUT: c_int = 2;
pub const AUDIO_DEVICE_FLAG_ALL: c_int = 3;

/// `OH_AudioStream_Result`
pub const AUDIOSTREAM_SUCCESS: c_int = 0;

/// `OH_AudioCommon_Result`
pub const AUDIOCOMMON_RESULT_SUCCESS: c_int = 0;

/// `OH_AudioDeviceDescriptorArray`
#[repr(C)]
pub struct AudioDeviceDescriptorArray {
    pub size: u32,
    pub descriptors: *mut *mut AudioDeviceDescriptor,
}

/// `OH_AudioRenderer_Callbacks_Struct`
#[repr(C)]
pub struct AudioRendererCallbacks {
    pub write_data:
        Option<extern "C" fn(*mut AudioRenderer, *mut c_void, *mut c_void, c_int) -> c_int>,
    pub stream_event: Option<extern "C" fn(*mut AudioRenderer, *mut c_void, c_int) -> c_int>,
    pub interrupt_event: Option<extern "C" fn(*mut AudioRenderer, *mut c_void, c_int, c_int) -> c_int>,
    pub error: Option<extern "C" fn(*mut AudioRenderer, *mut c_void, c_int) -> c_int>,
}

/// `OH_AudioCapturer_Callbacks_Struct`
#[repr(C)]
pub struct AudioCapturerCallbacks {
    pub read_data:
        Option<extern "C" fn(*mut AudioCapturer, *mut c_void, *mut c_void, c_int) -> c_int>,
    pub stream_event: Option<extern "C" fn(*mut AudioCapturer, *mut c_void, c_int) -> c_int>,
    pub interrupt_event: Option<extern "C" fn(*mut AudioCapturer, *mut c_void, c_int, c_int) -> c_int>,
    pub error: Option<extern "C" fn(*mut AudioCapturer, *mut c_void, c_int) -> c_int>,
}

// SAFETY: these symbols come from the OHOS audio NDK and are safe to declare;
// every call site below is wrapped in `unsafe` with the pointer preconditions
// documented by the SDK headers.
#[link(name = "ohaudio")]
unsafe extern "C" {
    pub fn OH_AudioStreamBuilder_Create(
        builder: *mut *mut AudioStreamBuilder,
        stream_type: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_Destroy(builder: *mut AudioStreamBuilder) -> c_int;
    pub fn OH_AudioStreamBuilder_SetSamplingRate(builder: *mut AudioStreamBuilder, rate: c_int)
    -> c_int;
    pub fn OH_AudioStreamBuilder_SetChannelCount(
        builder: *mut AudioStreamBuilder,
        channel_count: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetSampleFormat(
        builder: *mut AudioStreamBuilder,
        format: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetEncodingType(
        builder: *mut AudioStreamBuilder,
        encoding_type: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetRendererInfo(
        builder: *mut AudioStreamBuilder,
        usage: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetCapturerInfo(
        builder: *mut AudioStreamBuilder,
        source_type: c_int,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetRendererCallback(
        builder: *mut AudioStreamBuilder,
        callbacks: *mut AudioRendererCallbacks,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_SetCapturerCallback(
        builder: *mut AudioStreamBuilder,
        callbacks: *mut AudioCapturerCallbacks,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_GenerateRenderer(
        builder: *mut AudioStreamBuilder,
        renderer: *mut *mut AudioRenderer,
    ) -> c_int;
    pub fn OH_AudioStreamBuilder_GenerateCapturer(
        builder: *mut AudioStreamBuilder,
        capturer: *mut *mut AudioCapturer,
    ) -> c_int;

    pub fn OH_AudioRenderer_Start(renderer: *mut AudioRenderer) -> c_int;
    pub fn OH_AudioRenderer_Stop(renderer: *mut AudioRenderer) -> c_int;
    pub fn OH_AudioRenderer_Flush(renderer: *mut AudioRenderer) -> c_int;
    pub fn OH_AudioRenderer_Release(renderer: *mut AudioRenderer) -> c_int;

    pub fn OH_AudioCapturer_Start(capturer: *mut AudioCapturer) -> c_int;
    pub fn OH_AudioCapturer_Stop(capturer: *mut AudioCapturer) -> c_int;
    pub fn OH_AudioCapturer_Flush(capturer: *mut AudioCapturer) -> c_int;
    pub fn OH_AudioCapturer_Release(capturer: *mut AudioCapturer) -> c_int;

    pub fn OH_AudioManager_GetAudioRoutingManager(
        routing_manager: *mut *mut AudioRoutingManager,
    ) -> c_int;
    pub fn OH_AudioRoutingManager_GetDevices(
        routing_manager: *mut AudioRoutingManager,
        device_flag: c_int,
        devices: *mut *mut AudioDeviceDescriptorArray,
    ) -> c_int;
    pub fn OH_AudioRoutingManager_ReleaseDevices(
        routing_manager: *mut AudioRoutingManager,
        devices: *mut AudioDeviceDescriptorArray,
    ) -> c_int;
    pub fn OH_AudioDeviceDescriptor_GetDeviceType(
        descriptor: *mut AudioDeviceDescriptor,
        device_type: *mut c_int,
    ) -> c_int;
    pub fn OH_AudioDeviceDescriptor_GetDeviceId(
        descriptor: *mut AudioDeviceDescriptor,
        id: *mut u32,
    ) -> c_int;
    pub fn OH_AudioDeviceDescriptor_GetDeviceName(
        descriptor: *mut AudioDeviceDescriptor,
        name: *mut *mut c_char,
    ) -> c_int;
    pub fn OH_AudioDeviceDescriptor_GetDeviceDisplayName(
        descriptor: *mut AudioDeviceDescriptor,
        display_name: *mut *mut c_char,
    ) -> c_int;

    pub fn OH_AudioManager_GetAudioDeviceEnhanceManager(
        enhance_manager: *mut *mut AudioDeviceEnhanceManager,
    ) -> c_int;
    pub fn OH_AudioDeviceEnhanceManager_IsEnhancedRoutingSupported(
        enhance_manager: *mut AudioDeviceEnhanceManager,
        supported: *mut bool,
    ) -> c_int;
    pub fn OH_AudioDeviceEnhanceManager_SelectOutputDeviceForAudioRenderer(
        enhance_manager: *mut AudioDeviceEnhanceManager,
        renderer: *mut AudioRenderer,
        descriptor: *mut AudioDeviceDescriptor,
    ) -> c_int;
    pub fn OH_AudioDeviceEnhanceManager_SelectInputDeviceForAudioCapturer(
        enhance_manager: *mut AudioDeviceEnhanceManager,
        capturer: *mut AudioCapturer,
        descriptor: *mut AudioDeviceDescriptor,
    ) -> c_int;
}

/// Maps a device type to the label the system settings app uses, for builds
/// where the device descriptor carries neither a name nor a display name.
pub fn device_type_label(device_type: c_int) -> String {
    let label = match device_type {
        1 => "听筒",
        2 => "扬声器",
        3 => "有线耳机",
        4 => "有线耳机",
        7 => "蓝牙耳机",
        8 => "蓝牙音频设备",
        15 => "麦克风",
        22 => "USB 耳机",
        23 => "显示设备",
        24 => "投屏设备",
        25 => "USB 音频设备",
        26 => "音频配件",
        27 => "HDMI 设备",
        28 => "数字音频设备",
        30 => "助听器",
        31 => "星闪设备",
        1000 => "默认设备",
        _ => "音频设备",
    };
    label.to_string()
}

/// Maps a device type to `(supports_input, supports_output)`.
pub fn device_type_capabilities(device_type: c_int) -> (bool, bool) {
    // Values from `OH_AudioDevice_Type`.
    const EARPIECE: c_int = 1;
    const SPEAKER: c_int = 2;
    const WIRED_HEADSET: c_int = 3;
    const WIRED_HEADPHONES: c_int = 4;
    const BLUETOOTH_SCO: c_int = 7;
    const BLUETOOTH_A2DP: c_int = 8;
    const MIC: c_int = 15;
    const USB_HEADSET: c_int = 22;
    const DISPLAY_PORT: c_int = 23;
    const REMOTE_CAST: c_int = 24;
    const USB_DEVICE: c_int = 25;
    const ACCESSORY: c_int = 26;
    const HDMI: c_int = 27;
    const LINE_DIGITAL: c_int = 28;
    const HEARING_AID: c_int = 30;
    const NEARLINK: c_int = 31;
    const DEFAULT: c_int = 1000;

    match device_type {
        MIC => (true, false),
        EARPIECE | SPEAKER | WIRED_HEADPHONES | BLUETOOTH_A2DP | DISPLAY_PORT | REMOTE_CAST
        | HDMI | LINE_DIGITAL | HEARING_AID | NEARLINK => (false, true),
        // `DEFAULT` does not carry a direction; report both so the system default
        // stays selectable in the input and output dropdowns.
        WIRED_HEADSET | BLUETOOTH_SCO | USB_HEADSET | USB_DEVICE | ACCESSORY | DEFAULT => {
            (true, true)
        }
        _ => (false, false),
    }
}
