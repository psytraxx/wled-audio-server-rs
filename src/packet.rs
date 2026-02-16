use std::net::{SocketAddr, UdpSocket};

/// V2 AudioSync packet for WLED AudioReactive (44 bytes, little-endian).
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

pub struct UdpSender {
    socket: UdpSocket,
    target: SocketAddr,
    frame_counter: u8,
}

impl UdpSender {
    pub fn new(target_ip: &str, port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let target: SocketAddr = format!("{target_ip}:{port}").parse().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, e)
        })?;
        Ok(Self {
            socket,
            target,
            frame_counter: 0,
        })
    }

    pub fn send(&mut self, packet: &AudioSyncPacketV2) -> std::io::Result<()> {
        let bytes = packet.to_bytes(self.frame_counter);
        self.socket.send_to(&bytes, self.target)?;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        Ok(())
    }
}
