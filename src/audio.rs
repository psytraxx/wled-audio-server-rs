#[cfg(target_os = "linux")]
extern crate libc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BuildStreamError, Device, FromSample, InputCallbackInfo, Sample, SampleFormat, Stream};
use dialoguer::Select;
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

/// Presents an interactive chooser over all cpal input devices.
///
/// Works on all platforms. On macOS, users should have BlackHole (or similar)
/// installed so that a loopback device appears in the list.
///
/// Returns `Some(device_name)` on success, `None` if no devices are found or
/// the user cancels.
pub fn choose_input_device() -> Option<String> {
    let host = cpal::default_host();
    let devices: Vec<Device> = host.input_devices().ok()?.collect();

    // Probe each device for a usable input config while suppressing ALSA/JACK
    // error spam that leaks to stderr when probing unsupported plugin devices.
    let usable: Vec<String> = with_stderr_suppressed(|| {
        devices
            .into_iter()
            .filter_map(|d| {
                d.default_input_config().ok()?;
                #[allow(deprecated)]
                let name = d.name().ok()?;
                // Exclude the ALSA null sink — it captures silence only.
                if name == "null" {
                    return None;
                }
                Some(name)
            })
            .collect()
    });

    if usable.is_empty() {
        eprintln!("No input devices found.");
        return None;
    }

    // Default cursor to "default" if present, else "pulse", else first item.
    let default_idx = usable
        .iter()
        .position(|n| n == "default")
        .or_else(|| usable.iter().position(|n| n == "pulse"))
        .unwrap_or(0);

    let selection = Select::new()
        .with_prompt("Select audio input device")
        .items(&usable)
        .default(default_idx)
        .interact()
        .ok()?;

    Some(usable[selection].clone())
}

/// Temporarily redirects stderr to /dev/null for the duration of `f`.
///
/// Used to suppress ALSA/JACK error messages that leak to the terminal
/// when probing devices that don't support audio input.
#[cfg(target_os = "linux")]
fn with_stderr_suppressed<F: FnOnce() -> T, T>(f: F) -> T {
    unsafe {
        let devnull = libc::open(b"/dev/null\0".as_ptr() as *const libc::c_char, libc::O_WRONLY);
        let saved = libc::dup(libc::STDERR_FILENO);
        libc::dup2(devnull, libc::STDERR_FILENO);
        libc::close(devnull);
        let result = f();
        libc::dup2(saved, libc::STDERR_FILENO);
        libc::close(saved);
        result
    }
}

#[cfg(not(target_os = "linux"))]
fn with_stderr_suppressed<F: FnOnce() -> T, T>(f: F) -> T {
    f()
}

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
/// let (_stream, sample_rate, rx, _drop_counter) = open_capture_stream(Some("BlackHole 2ch"))?;
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
