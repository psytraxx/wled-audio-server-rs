use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, Stream};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

/// Print all available input devices.
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

/// Find an input device by substring match, or auto-detect a monitor device.
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

/// Open a capture stream. Returns (Stream, sample_rate, Receiver<Vec<f32>>).
/// The receiver yields chunks of mono f32 samples.
pub fn open_capture_stream(
    device_hint: Option<&str>,
) -> Result<(Stream, u32, Receiver<Vec<f32>>), String> {
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

    let (tx, rx): (SyncSender<Vec<f32>>, Receiver<Vec<f32>>) = sync_channel(4);

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), channels, tx),
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), channels, tx),
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), channels, tx),
        fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
    }
    .map_err(|e| format!("Failed to build stream: {e}"))?;

    stream.play().map_err(|e| format!("Failed to start stream: {e}"))?;

    Ok((stream, sample_rate, rx))
}

fn build_stream<T: cpal::SizedSample + Send + 'static>(
    device: &Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: SyncSender<Vec<f32>>,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    sum / channels as f32
                })
                .collect();
            // Drop samples if the consumer can't keep up (bounded channel)
            let _ = tx.try_send(mono);
        },
        |err| {
            eprintln!("Audio stream error: {err}");
        },
        None,
    )
}
