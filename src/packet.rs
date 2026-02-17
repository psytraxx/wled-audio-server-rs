use if_addrs::{get_if_addrs, IfAddr};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

/// V2 AudioSync packet for WLED AudioReactive (44 bytes, little-endian).
///
/// This structure represents the WLED AudioSync V2 protocol packet format.
/// When serialized, it produces exactly 44 bytes suitable for UDP transmission
/// to WLED devices running AudioReactive firmware.
///
/// # Packet Format
/// ```text
/// Offset  Size  Type      Field
/// 0       6     [u8;6]    header = "00002\0"
/// 6       2     [u8;2]    pressure (unused, zero)
/// 8       4     f32       sampleRaw (0..255)
/// 12      4     f32       sampleSmth (0..255)
/// 16      1     u8        samplePeak (0=no beat, 1=beat)
/// 17      1     u8        frameCounter (0..255 rolling)
/// 18      16    [u8;16]   fftResult (16 bins, each 0..255)
/// 34      2     u16       zeroCrossingCount
/// 36      4     f32       FFT_Magnitude
/// 40      4     f32       FFT_MajorPeak (Hz)
/// ```
pub struct AudioSyncPacketV2 {
    pub sample_raw: f32,
    pub sample_smth: f32,
    pub sample_peak: u8,
    pub fft_result: [u8; 16],
    pub zero_crossing_count: u16,
    pub fft_magnitude: f32,
    pub fft_major_peak: f32,
}

impl AudioSyncPacketV2 {
    /// Serializes the packet to a 44-byte array in WLED V2 format.
    ///
    /// # Arguments
    /// * `frame_counter` - Rolling frame counter (0-255) for packet sequencing
    ///
    /// # Returns
    /// A 44-byte array ready for UDP transmission, with all fields in little-endian byte order.
    pub fn to_bytes(&self, frame_counter: u8) -> [u8; 44] {
        let mut buf = [0u8; 44];

        // Header: "00002\0"
        buf[0] = b'0';
        buf[1] = b'0';
        buf[2] = b'0';
        buf[3] = b'0';
        buf[4] = b'2';
        buf[5] = 0;

        // Pressure (fixed-point, unused — leave zero)
        // buf[6..8] = [0, 0]

        // sampleRaw (f32 LE)
        buf[8..12].copy_from_slice(&self.sample_raw.to_le_bytes());

        // sampleSmth (f32 LE)
        buf[12..16].copy_from_slice(&self.sample_smth.to_le_bytes());

        // samplePeak (u8)
        buf[16] = self.sample_peak;

        // frameCounter (u8)
        buf[17] = frame_counter;

        // fftResult (16 bytes)
        buf[18..34].copy_from_slice(&self.fft_result);

        // zeroCrossingCount (u16 LE)
        buf[34..36].copy_from_slice(&self.zero_crossing_count.to_le_bytes());

        // FFT_Magnitude (f32 LE)
        buf[36..40].copy_from_slice(&self.fft_magnitude.to_le_bytes());

        // FFT_MajorPeak (f32 LE)
        buf[40..44].copy_from_slice(&self.fft_major_peak.to_le_bytes());

        buf
    }
}

/// UDP packet sender with automatic frame counter management.
///
/// Manages a UDP socket and maintains a rolling frame counter
/// for AudioSync packet transmission to WLED devices.
pub struct UdpSender {
    socket: UdpSocket,
    targets: Vec<SocketAddr>,
    frame_counter: u8,
}

impl UdpSender {
    /// Creates a new UDP sender bound to an ephemeral port.
    ///
    /// # Arguments
    /// * `port` - Target UDP port (typically 11988 for WLED AudioReactive)
    ///
    /// # Returns
    /// * `Ok(UdpSender)` - Ready-to-use sender with frame counter initialized to 0
    /// * `Err(io::Error)` - If socket setup fails
    pub fn new(port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_broadcast(true)?;
        let targets = discover_broadcast_targets(port);
        Ok(Self {
            socket,
            targets,
            frame_counter: 0,
        })
    }

    pub fn targets(&self) -> &[SocketAddr] {
        &self.targets
    }

    /// Sends an AudioSync packet to the target WLED device.
    ///
    /// Automatically increments the internal frame counter after each send.
    ///
    /// # Arguments
    /// * `packet` - The packet to serialize and transmit
    ///
    /// # Returns
    /// * `Ok(())` - Packet sent successfully
    /// * `Err(io::Error)` - If UDP transmission fails
    pub fn send(&mut self, packet: &AudioSyncPacketV2) -> std::io::Result<()> {
        let bytes = packet.to_bytes(self.frame_counter);
        let mut last_error = None;
        let mut any_sent = false;

        for target in &self.targets {
            match self.socket.send_to(&bytes, target) {
                Ok(_) => any_sent = true,
                Err(e) => last_error = Some(e),
            }
        }

        if !any_sent {
            return Err(last_error.unwrap_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "No broadcast targets available",
                )
            }));
        }

        self.frame_counter = self.frame_counter.wrapping_add(1);
        Ok(())
    }
}

fn discover_broadcast_targets(port: u16) -> Vec<SocketAddr> {
    let mut unique = HashSet::new();
    unique.insert(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(255, 255, 255, 255),
        port,
    )));

    if let Ok(ifaces) = get_if_addrs() {
        for iface in ifaces {
            if let IfAddr::V4(v4) = iface.addr {
                if v4.ip.is_loopback() {
                    continue;
                }

                let ip_u32 = u32::from(v4.ip);
                let mask_u32 = u32::from(v4.netmask);
                let broadcast = Ipv4Addr::from(ip_u32 | !mask_u32);
                unique.insert(SocketAddr::V4(SocketAddrV4::new(broadcast, port)));
            }
        }
    }

    unique.into_iter().collect()
}
