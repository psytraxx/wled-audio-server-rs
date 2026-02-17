use clap::Parser;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
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

    /// Enable verbose debug output
    #[arg(short, long)]
    verbose: bool,
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
    let (_stream, sample_rate, rx, drop_counter) = match open_capture_stream(args.device.as_deref())
    {
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
    if args.verbose {
        println!("Verbose mode enabled");
        println!(
            "DSP: FFT size 2048, 50% overlap, ~{:.1} frames/sec",
            sample_rate as f32 / 1024.0
        );
    }
    println!("Press Ctrl+C to stop.");

    let mut dsp = dsp::DspProcessor::new(sample_rate);
    let mut last_drop_check = Instant::now();
    let mut last_drop_count: u64 = 0;
    let mut packet_count: u64 = 0;
    let mut last_verbose_log = Instant::now();

    // Main loop
    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(samples) => {
                if args.verbose && last_verbose_log.elapsed() >= Duration::from_millis(500) {
                    println!(
                        "[Verbose] Received {} samples, buffer at {} samples",
                        samples.len(),
                        samples.len()
                    );
                    last_verbose_log = Instant::now();
                }

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
                    } else if args.verbose {
                        packet_count += 1;
                        if packet_count % 100 == 0 {
                            println!(
                                "[Verbose] Sent packet #{}: raw={:.1}, smth={:.1}, peak={}, mag={:.1}, freq={:.0}Hz, bins=[{},{},{},...]",
                                packet_count,
                                frame.sample_raw,
                                frame.sample_smth,
                                frame.sample_peak,
                                frame.fft_magnitude,
                                frame.fft_major_peak,
                                frame.fft_result[0],
                                frame.fft_result[1],
                                frame.fft_result[2],
                            );
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Check for dropped frames every 5 seconds
                if last_drop_check.elapsed() >= Duration::from_secs(5) {
                    let current_drops = drop_counter.load(Ordering::Relaxed);
                    let new_drops = current_drops - last_drop_count;
                    if new_drops > 0 {
                        eprintln!(
                            "Warning: Dropped {} audio chunks in the last 5 seconds (total: {})",
                            new_drops, current_drops
                        );
                    }
                    last_drop_count = current_drops;
                    last_drop_check = Instant::now();
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Final drop count report
    let total_drops = drop_counter.load(Ordering::Relaxed);
    if total_drops > 0 {
        eprintln!("Total audio chunks dropped during session: {}", total_drops);
    }

    println!("\nShutting down.");
}
