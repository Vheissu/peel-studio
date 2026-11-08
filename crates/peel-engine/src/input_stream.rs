//! Opening audio input devices with cpal.
//!
//! [`list_input_devices`] enumerates what the system offers. The engine
//! handle opens one with [`EngineHandle::open_input`](crate::EngineHandle::open_input):
//! the stream's callback runs an [`InputProcessor`](crate::input::InputProcessor)
//! (see [`input`](crate::input) for what it does and the rules it follows).
//! Opening an input is what makes the operating system ask for microphone
//! permission, so the engine never does it on its own at startup.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
};

use crate::handle::{common_rates, device_name, match_device, merge_ranges};
use crate::input::{CallbackTiming, InputFailure, InputProcessor, InputShared};

/// What to open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputRequest {
    /// Device name or id from [`list_input_devices`]; `None` is the system
    /// default input. A device that cannot be found is an error (a take must
    /// never silently come from a different microphone).
    pub device: Option<String>,
    /// Preferred sample rate. The engine's output rate is tried first when
    /// this is `None`, so monitoring works; the device's own rate otherwise.
    pub sample_rate: Option<u32>,
    /// Preferred device buffer in frames.
    pub buffer_frames: Option<u32>,
}

impl InputRequest {
    /// The input [`EngineConfig::input_device`] names, with the output's
    /// buffer size.
    pub fn from_config(config: &crate::EngineConfig) -> InputRequest {
        InputRequest {
            device: config.input_device.clone().filter(|d| !d.trim().is_empty()),
            sample_rate: None,
            buffer_frames: config.buffer_frames,
        }
    }
}

/// An opened input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputInfo {
    pub device_name: String,
    pub sample_rate: u32,
    /// Channels the device delivers (all of them are available to choose
    /// from).
    pub channels: u16,
    /// Device buffer in frames if the stream reports it.
    pub buffer_frames: Option<u32>,
}

/// An audio input device the engine can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDeviceInfo {
    /// Human-readable name. Not guaranteed unique; see [`id`](Self::id).
    pub name: String,
    /// Backend identifier, stable across runs where the platform allows it.
    /// Either this or the name is accepted by [`InputRequest::device`].
    pub id: String,
    /// The system default input.
    pub is_default: bool,
    /// Most channels any supported format offers.
    pub max_channels: u16,
    /// Common rates (22.05 kHz to 192 kHz) the device supports.
    pub sample_rates: Vec<u32>,
    /// The device's preferred rate, if it reports one.
    pub default_sample_rate: Option<u32>,
}

/// Why input could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// The system has no input device.
    #[error(
        "No audio input device found. Connect a microphone or interface, then choose it again."
    )]
    NoDevice,
    /// The chosen device is not there any more.
    #[error("The audio input \"{0}\" was not found. Reconnect it or choose another input.")]
    NotFound(String),
    /// The operating system refused access.
    #[error(
        "Microphone access was denied. Allow Peel Studio under System Settings, Privacy & Security, Microphone (or your system's equivalent), then try again."
    )]
    PermissionDenied,
    /// The device is in use or cannot do what was asked.
    #[error("Could not open the audio input \"{device}\": {reason}")]
    Unavailable { device: String, reason: String },
}

/// Lists the input devices of the default audio host.
///
/// Enumeration queries every device and can take tens of milliseconds, so
/// call it when a settings panel opens, not every frame. It does not open any
/// device, so it never triggers a microphone permission prompt.
pub fn list_input_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|device| {
            let ranges: Vec<_> = device.supported_input_configs().ok()?.collect();
            if ranges.is_empty() {
                return None;
            }
            let id = device.id().ok();
            let merged = merge_ranges(
                ranges
                    .iter()
                    .map(|r| (r.min_sample_rate(), r.max_sample_rate()))
                    .collect(),
            );
            Some(InputDeviceInfo {
                name: device_name(&device),
                is_default: id.is_some() && id == default_id,
                id: id.map(|i| i.to_string()).unwrap_or_default(),
                max_channels: ranges.iter().map(|r| r.channels()).max().unwrap_or(0),
                sample_rates: common_rates(&merged),
                default_sample_rate: device.default_input_config().ok().map(|c| c.sample_rate()),
            })
        })
        .collect()
}

fn find_input_device(host: &cpal::Host, wanted: &str) -> Option<cpal::Device> {
    let devices: Vec<cpal::Device> = host.input_devices().ok()?.collect();
    let keys: Vec<(String, String)> = devices
        .iter()
        .map(|d| {
            let id = d.id().map(|i| i.to_string()).unwrap_or_default();
            (id, device_name(d))
        })
        .collect();
    let pos = match_device(&keys, wanted)?;
    devices.into_iter().nth(pos)
}

fn map_build_error(device: &str, e: &cpal::Error) -> InputError {
    match e.kind() {
        ErrorKind::PermissionDenied => InputError::PermissionDenied,
        ErrorKind::DeviceNotAvailable => InputError::NotFound(device.to_string()),
        _ => InputError::Unavailable {
            device: device.to_string(),
            reason: e.to_string(),
        },
    }
}

/// An open cpal input stream and what it was opened with.
pub(crate) struct OpenedStream {
    pub stream: cpal::Stream,
    pub info: InputInfo,
}

/// Candidate stream formats, best first: the wanted rate, then the most
/// channels (so any channel can be picked), then the cheapest sample format.
struct Candidate {
    channels: u16,
    rate: u32,
    format: SampleFormat,
    buffer: SupportedBufferSize,
}

fn format_rank(f: SampleFormat) -> Option<u8> {
    Some(match f {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::F64 => 3,
        SampleFormat::U16 => 4,
        SampleFormat::I24 => 5,
        SampleFormat::U32 => 6,
        SampleFormat::U24 => 7,
        SampleFormat::I8 => 8,
        SampleFormat::U8 => 9,
        SampleFormat::I64 => 10,
        SampleFormat::U64 => 11,
        _ => return None,
    })
}

/// How many channels a stream opens with at most. Interfaces with more
/// inputs are still usable; the rest are simply not offered.
const MAX_STREAM_CHANNELS: u16 = 64;

/// Opens the requested device and starts its stream. `make_processor` builds
/// the processor for one attempt given the stream's sample rate and channel
/// count (each attempt gets fresh rings); `commit` is called with the attempt
/// that worked.
pub(crate) fn open_stream<P>(
    request: &InputRequest,
    output_rate: u32,
    shared: &Arc<InputShared>,
    mut make_processor: P,
) -> Result<OpenedStream, InputError>
where
    P: FnMut(u32, u16) -> InputProcessor,
{
    let host = cpal::default_host();
    let device = match request.device.as_deref().filter(|d| !d.is_empty()) {
        Some(wanted) => find_input_device(&host, wanted)
            .ok_or_else(|| InputError::NotFound(wanted.to_string()))?,
        None => host.default_input_device().ok_or(InputError::NoDevice)?,
    };
    let name = device_name(&device);
    let unavailable = |reason: String| InputError::Unavailable {
        device: name.clone(),
        reason,
    };

    let default_rate = device.default_input_config().ok().map(|c| c.sample_rate());
    let wanted_rate = request.sample_rate.unwrap_or(output_rate);
    let ranges: Vec<_> = device
        .supported_input_configs()
        .map_err(|e| map_build_error(&name, &e))?
        .collect();

    let mut candidates: Vec<(u8, u16, u8, Candidate)> = ranges
        .iter()
        .filter_map(|r| {
            let fmt_rank = format_rank(r.sample_format())?;
            if r.channels() == 0 {
                return None;
            }
            let (min, max) = (r.min_sample_rate(), r.max_sample_rate());
            let rate_ok = (min..=max).contains(&wanted_rate);
            let rate = if rate_ok {
                wanted_rate
            } else {
                default_rate
                    .filter(|d| (min..=max).contains(d))
                    .unwrap_or_else(|| wanted_rate.clamp(min, max))
            };
            let channels = r.channels().min(MAX_STREAM_CHANNELS);
            Some((
                u8::from(!rate_ok),
                u16::MAX - channels,
                fmt_rank,
                Candidate {
                    channels,
                    rate,
                    format: r.sample_format(),
                    buffer: *r.buffer_size(),
                },
            ))
        })
        .collect();
    candidates.sort_by_key(|(a, b, c, _)| (*a, *b, *c));
    if candidates.is_empty() {
        return Err(unavailable("it offers no usable input format".into()));
    }

    let mut last_error: Option<InputError> = None;
    for (_, _, _, cand) in candidates.iter().take(4) {
        let requested = request.buffer_frames.map(|n| match cand.buffer {
            SupportedBufferSize::Range { min, max } => n.clamp(min, max),
            SupportedBufferSize::Unknown => n,
        });
        let sizes = match requested {
            Some(n) => [BufferSize::Fixed(n), BufferSize::Default],
            None => [BufferSize::Default, BufferSize::Default],
        };
        for (i, size) in sizes.into_iter().enumerate() {
            if i == 1 && sizes[0] == sizes[1] {
                break;
            }
            let config = StreamConfig {
                channels: cand.channels,
                sample_rate: cand.rate,
                buffer_size: size,
            };
            let processor = make_processor(cand.rate, cand.channels);
            match build_stream(&device, cand.format, config, processor, shared.clone()) {
                Ok(stream) => {
                    if let Err(e) = stream.play() {
                        last_error = Some(map_build_error(&name, &e));
                        continue;
                    }
                    let buffer_frames = stream.buffer_size().ok().or(match size {
                        BufferSize::Fixed(n) => Some(n),
                        BufferSize::Default => None,
                    });
                    return Ok(OpenedStream {
                        stream,
                        info: InputInfo {
                            device_name: name,
                            sample_rate: cand.rate,
                            channels: cand.channels,
                            buffer_frames,
                        },
                    });
                }
                Err(e) => {
                    let mapped = map_build_error(&name, &e);
                    if mapped == InputError::PermissionDenied {
                        return Err(mapped);
                    }
                    last_error = Some(mapped);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| unavailable("no format could be opened".into())))
}

fn build_stream(
    device: &cpal::Device,
    format: SampleFormat,
    config: StreamConfig,
    processor: InputProcessor,
    shared: Arc<InputShared>,
) -> Result<cpal::Stream, cpal::Error> {
    let on_error = move |err: cpal::Error| match err.kind() {
        ErrorKind::Xrun => {
            shared.note_xrun();
            log::warn!("audio input xrun: {err}");
        }
        ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
            log::warn!("audio input stream: {err}");
        }
        ErrorKind::DeviceNotAvailable => {
            log::error!("audio input lost: {err}");
            shared.fail(InputFailure::DeviceLost, err.to_string());
        }
        _ => {
            log::error!("audio input stream error: {err}");
            shared.fail(InputFailure::StreamError, err.to_string());
        }
    };
    macro_rules! build {
        ($t:ty) => {{
            let cb = make_callback::<$t>(processor);
            device.build_input_stream(config, cb, on_error, None)
        }};
    }
    match format {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::F64 => build!(f64),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U8 => build!(u8),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U64 => build!(u64),
        other => Err(cpal::Error::with_message(
            ErrorKind::UnsupportedConfig,
            format!("unsupported sample format {other}"),
        )),
    }
}

/// The input data callback for sample type `T`.
fn make_callback<T>(
    mut processor: InputProcessor,
) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    move |data: &[T], info: &cpal::InputCallbackInfo| {
        crate::rt::guard(|| {
            let ts = info.timestamp();
            processor.process(
                data,
                CallbackTiming {
                    callback_ns: ts.callback.as_nanos() as u64,
                    capture_ns: ts.capture.as_nanos() as u64,
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_say_what_to_do() {
        assert!(InputError::NoDevice.to_string().contains("Connect"));
        assert!(
            InputError::NotFound("USB Mic".into())
                .to_string()
                .contains("USB Mic")
        );
        assert!(
            InputError::PermissionDenied
                .to_string()
                .contains("Microphone")
        );
    }

    /// Opens real hardware. Run with `--ignored` on a machine with a
    /// microphone; macOS asks for permission the first time.
    #[test]
    #[ignore = "needs an audio input device and microphone permission"]
    fn the_default_input_delivers_callbacks_and_a_meter() {
        use crate::{EngineConfig, EngineHandle};
        let mut h = EngineHandle::start_null(EngineConfig::default()).unwrap();
        let info = h.open_input(&InputRequest::default()).expect("open input");
        assert!(info.channels > 0 && info.sample_rate > 0, "{info:?}");
        assert_eq!(h.input_info(), Some(&info));
        std::thread::sleep(std::time::Duration::from_millis(600));
        let shared = h.input();
        assert!(shared.callbacks() > 5, "callbacks: {}", shared.callbacks());
        assert!(shared.failure().is_none(), "{:?}", shared.last_error());
        // The meter reports something finite, silence included.
        let (l, r) = shared.take_peak();
        assert!(l.is_finite() && r.is_finite());
        h.close_input();
        assert!(!h.input().is_open());
    }

    #[test]
    #[ignore = "needs an audio input device and microphone permission"]
    fn each_listed_input_can_be_opened_by_name() {
        use crate::{EngineConfig, EngineHandle};
        for device in list_input_devices() {
            let mut h = EngineHandle::start_null(EngineConfig::default()).unwrap();
            let info = h
                .open_input(&InputRequest {
                    device: Some(device.name.clone()),
                    ..InputRequest::default()
                })
                .unwrap_or_else(|e| panic!("{}: {e}", device.name));
            assert_eq!(info.device_name, device.name);
        }
    }

    #[test]
    fn a_missing_input_is_an_error_not_a_substitute() {
        use crate::{EngineConfig, EngineHandle};
        let mut h = EngineHandle::start_null(EngineConfig::default()).unwrap();
        let err = h
            .open_input(&InputRequest {
                device: Some("No Such Interface".into()),
                ..InputRequest::default()
            })
            .unwrap_err();
        assert_eq!(err, InputError::NotFound("No Such Interface".into()));
        assert!(!h.input().is_open());
    }

    #[test]
    fn enumerating_input_devices_never_panics() {
        // Content depends on the machine: CI runners often have none.
        let devices = list_input_devices();
        assert!(devices.iter().filter(|d| d.is_default).count() <= 1);
        for d in &devices {
            assert!(!d.name.is_empty());
            assert!(d.max_channels > 0);
        }
    }
}
