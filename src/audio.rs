use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BuildStreamError, Device, FromSample, InputCallbackInfo, Sample, SampleFormat, Stream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

pub type CaptureStreamHandle = (Stream, u32, Receiver<Vec<f32>>, Arc<AtomicU64>);

/// Size of the bounded audio sample channel.
///
/// This determines how many chunks of samples can be queued between the audio
/// capture callback and the DSP processor. A larger value provides more buffering
/// against processing delays but increases latency. If the consumer cannot keep up,
/// samples will be dropped (using try_send).
///
/// Value of 8 provides good balance between latency and dropout prevention.
/// At 48kHz with typical chunk sizes, this represents ~10-20ms of buffering.
const AUDIO_CHANNEL_SIZE: usize = 8;

/// Lists all available audio input devices to stdout.
///
/// For each device, displays its index, name, number of channels,
/// sample rate, and sample format. If a device cannot be queried,
/// it will show "no config" instead of configuration details.
///
/// # Example
/// ```text
/// Available input devices:
///   [0] Built-in Audio (2ch 48000Hz F32)
///   [1] pulse (2ch 48000Hz I16)
/// ```
pub fn list_devices() {
    let host = cpal::default_host();
    let devices = match host.input_devices() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error listing devices: {e}");
            return;
        }
    };
    println!("Available input devices:");
    for (i, dev) in devices.enumerate() {
        #[allow(deprecated)]
        let name = dev.name().unwrap_or_else(|_| "<unknown>".into());
        let default_cfg = dev.default_input_config();
        let info = match default_cfg {
            Ok(cfg) => format!(
                "{}ch {}Hz {:?}",
                cfg.channels(),
                cfg.sample_rate(),
                cfg.sample_format()
            ),
            Err(_) => "no config".into(),
        };
        println!("  [{i}] {name} ({info})");
    }
}

/// Finds an audio input device by name substring match, or auto-detects a monitor device.
///
/// # Arguments
/// * `name_hint` - Optional substring to match against device names (case-insensitive).
///   If `None`, attempts to auto-detect a device with "monitor" in its name.
///
/// # Returns
/// * `Some(Device)` if a matching device is found
/// * `None` if no device matches, with helpful error messages printed to stderr
///
/// # Notes
/// On PipeWire/PulseAudio systems, monitor devices may not be visible via ALSA.
/// Users should set the `PULSE_SOURCE` environment variable or use the `-d` flag.
fn find_device(name_hint: Option<&str>) -> Option<Device> {
    let host = cpal::default_host();
    let devices: Vec<Device> = host.input_devices().ok()?.collect();

    if let Some(hint) = name_hint {
        let hint_lower = hint.to_lowercase();
        for dev in &devices {
            #[allow(deprecated)]
            if let Ok(name) = dev.name() {
                if name.to_lowercase().contains(&hint_lower) {
                    return Some(dev.clone());
                }
            }
        }
        eprintln!("No device matching '{hint}' found.");
        return None;
    }

    // Auto-detect: prefer device with "monitor" in the name
    for dev in &devices {
        #[allow(deprecated)]
        if let Ok(name) = dev.name() {
            if name.to_lowercase().contains("monitor") {
                return Some(dev.clone());
            }
        }
    }

    eprintln!("No monitor device found automatically.");
    eprintln!("Hint: set PULSE_SOURCE=<monitor_name> or use -d to specify a device.");
    eprintln!("Use --list-devices to see available devices.");
    None
}

/// Opens an audio capture stream and returns a channel receiver for audio samples.
///
/// # Arguments
/// * `device_hint` - Optional device name substring for device selection.
///   If `None`, auto-detects a monitor device.
///
/// # Returns
/// * `Ok((Stream, sample_rate, Receiver<Vec<f32>>, Arc<AtomicU64>))` - A tuple containing:
///   - The active audio stream (must be kept alive)
///   - Sample rate in Hz
///   - Channel receiver that yields mono f32 sample chunks
///   - Atomic counter for dropped sample chunks (for monitoring)
/// * `Err(String)` - Error description if device cannot be opened
///
/// # Notes
/// - Audio is automatically downmixed from stereo/multi-channel to mono
/// - Uses a bounded channel (size 4) that drops samples if consumer is slow
/// - Supports F32, I16, and U16 sample formats
/// - The Stream must remain in scope for capture to continue
///
/// # Example
/// ```no_run
/// use wled_audio_server::audio::open_capture_stream;
///
/// let (_stream, sample_rate, rx, _drop_counter) = open_capture_stream(Some("pulse"))?;
/// while let Ok(samples) = rx.recv() {
///     // Process samples...
/// }
/// # Ok::<(), String>(())
/// ```
pub fn open_capture_stream(device_hint: Option<&str>) -> Result<CaptureStreamHandle, String> {
    let device = find_device(device_hint).ok_or("Could not find audio device")?;
    #[allow(deprecated)]
    let dev_name = device.name().unwrap_or_else(|_| "<unknown>".into());

    let config = device
        .default_input_config()
        .map_err(|e| format!("No default input config: {e}"))?;

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;

    println!("Using device: {dev_name}");
    println!("Sample rate: {sample_rate} Hz, channels: {channels}");

    let (tx, rx): (SyncSender<Vec<f32>>, Receiver<Vec<f32>>) = sync_channel(AUDIO_CHANNEL_SIZE);
    let drop_counter = Arc::new(AtomicU64::new(0));

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            build_stream::<f32>(&device, &config.into(), channels, tx, drop_counter.clone())
        }
        SampleFormat::I16 => {
            build_stream::<i16>(&device, &config.into(), channels, tx, drop_counter.clone())
        }
        SampleFormat::U16 => {
            build_stream::<u16>(&device, &config.into(), channels, tx, drop_counter.clone())
        }
        fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
    }
    .map_err(|e| format!("Failed to build stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to start stream: {e}"))?;

    Ok((stream, sample_rate, rx, drop_counter))
}

fn build_stream<T: cpal::SizedSample + Send + 'static>(
    device: &Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: SyncSender<Vec<f32>>,
    drop_counter: Arc<AtomicU64>,
) -> Result<Stream, BuildStreamError>
where
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &InputCallbackInfo| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    sum / channels as f32
                })
                .collect();
            // Drop samples if the consumer can't keep up (bounded channel)
            if tx.try_send(mono).is_err() {
                drop_counter.fetch_add(1, Ordering::Relaxed);
            }
        },
        |err| {
            eprintln!("Audio stream error: {err}");
        },
        None,
    )
}
