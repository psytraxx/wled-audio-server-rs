use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wled_audio_server::audio::open_capture_stream;
use wled_audio_server::dsp;
use wled_audio_server::packet::{self, UdpSender};

#[derive(Parser)]
#[command(
    name = "wled-audio-server",
    about = "Stream system audio to WLED AudioReactive via UDP"
)]
struct Args {
    /// WLED target IP address
    #[arg(short = 't', long = "target", default_value = "192.168.178.63")]
    target: String,

    /// UDP port
    #[arg(short, long, default_value_t = 11988)]
    port: u16,

    /// List audio input devices and exit
    #[arg(short, long = "list-devices")]
    list_devices: bool,

    /// Audio device name (substring match); auto-selects "monitor" device
    #[arg(short, long)]
    device: Option<String>,
}

fn main() {
    let args = Args::parse();

    if args.list_devices {
        wled_audio_server::audio::list_devices();
        return;
    }

    // Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Failed to set Ctrl+C handler");

    // Open audio capture
    let (_stream, sample_rate, rx) = match open_capture_stream(args.device.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // UDP sender
    let mut sender = match UdpSender::new(&args.target, args.port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error creating UDP socket: {e}");
            std::process::exit(1);
        }
    };

    println!("Sending to {}:{}", args.target, args.port);
    println!("Press Ctrl+C to stop.");

    let mut dsp = dsp::DspProcessor::new(sample_rate);

    // Main loop
    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(samples) => {
                let frames = dsp.push_samples(&samples);
                for frame in frames {
                    let pkt = packet::AudioSyncPacketV2 {
                        sample_raw: frame.sample_raw,
                        sample_smth: frame.sample_smth,
                        sample_peak: frame.sample_peak,
                        fft_result: frame.fft_result,
                        zero_crossing_count: frame.zero_crossing_count,
                        fft_magnitude: frame.fft_magnitude,
                        fft_major_peak: frame.fft_major_peak,
                    };
                    if let Err(e) = sender.send(&pkt) {
                        eprintln!("UDP send error: {e}");
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("\nShutting down.");
}
