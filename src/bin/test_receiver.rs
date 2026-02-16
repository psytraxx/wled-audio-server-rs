use std::net::UdpSocket;

fn main() {
    let socket = UdpSocket::bind("0.0.0.0:11988").expect("Failed to bind socket");
    println!("Listening on 0.0.0.0:11988 for WLED packets...");

    let mut buf = [0u8; 128];
    for i in 0..5 {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                println!("\nPacket #{} from {}: {} bytes", i + 1, src, len);

                if len >= 6 {
                    let header = &buf[0..6];
                    print!("  Header: ");
                    for &b in header {
                        if b.is_ascii_graphic() || b == b' ' {
                            print!("{}", b as char);
                        } else {
                            print!("\\x{:02x}", b);
                        }
                    }
                    println!();

                    if &header[..5] == b"00002" && header[5] == 0 {
                        println!("  ✓ Valid V2 header");
                    } else {
                        println!("  ✗ Invalid header (expected '00002\\0')");
                    }
                }

                if len == 44 {
                    println!("  ✓ Correct packet size (44 bytes)");

                    // Sample values
                    let sample_raw = f32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
                    let sample_smth = f32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
                    let sample_peak = buf[16];
                    let frame_counter = buf[17];

                    println!(
                        "  sampleRaw: {:.2}, sampleSmth: {:.2}",
                        sample_raw, sample_smth
                    );
                    println!(
                        "  samplePeak: {}, frameCounter: {}",
                        sample_peak, frame_counter
                    );

                    // FFT bins
                    print!("  FFT bins: [");
                    for i in 0..16 {
                        print!("{}", buf[18 + i]);
                        if i < 15 {
                            print!(", ");
                        }
                    }
                    println!("]");
                } else {
                    println!("  ✗ Wrong packet size (expected 44)");
                }
            }
            Err(e) => {
                eprintln!("Error receiving: {}", e);
                break;
            }
        }
    }

    println!("\nValidation complete!");
}
