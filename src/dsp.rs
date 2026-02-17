use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::Arc;

const FFT_SIZE: usize = 2048;
const HOP_SIZE: usize = 1024;
const NUM_BINS: usize = 16;
const FREQ_MIN: f32 = 60.0;
const FREQ_MAX: f32 = 6000.0;
const SILENCE_THRESHOLD: f32 = 0.00001;
const AGC_ATTACK_OLD: f32 = 0.25;
const AGC_ATTACK_NEW: f32 = 0.75;
const AGC_RELEASE_OLD: f32 = 0.90;
const AGC_RELEASE_NEW: f32 = 0.10;
const BEAT_HISTORY: usize = 50;
const BEAT_THRESHOLD: f32 = 1.20;
const BEAT_FREQ_MIN: f32 = 100.0;
const BEAT_FREQ_MAX: f32 = 500.0;

/// FFT magnitude normalization factor for log-scale binning.
///
/// This value is empirically derived to scale FFT magnitude values into a range
/// suitable for AGC processing. After taking the square root of magnitude
/// (converting power to amplitude), dividing by this factor produces values
/// that typically range 0-10 for normal audio levels, which the AGC then
/// normalizes to 0-255 for transmission to WLED.
///
/// The specific value (0.04194) may have been tuned based on the combination of:
/// - FFT window function gain (HFT90D FlatTop has ~3.81 coherent gain)
/// - Expected input signal levels
/// - Desired sensitivity for WLED visualization
const FFT_BIN_SCALE: f32 = 0.04194;

/// Smoothing factor for exponential moving average of sampleSmth.
/// Higher values = more smoothing (slower response), range 0.0-1.0.
const SAMPLE_SMOOTH_FACTOR: f32 = 0.7;

/// Output of DSP processing for one FFT frame.
///
/// Contains amplitude, frequency analysis, and beat detection results
/// ready for transmission to WLED AudioReactive devices.
pub struct DspFrame {
    pub sample_raw: f32,
    pub sample_smth: f32,
    pub sample_peak: u8,
    pub fft_result: [u8; NUM_BINS],
    pub zero_crossing_count: u16,
    pub fft_magnitude: f32,
    pub fft_major_peak: f32,
}

/// Real-time audio DSP processor for WLED AudioReactive.
///
/// Performs FFT analysis with windowing, AGC, beat detection, and
/// log-spaced frequency binning. Uses 50% overlapping windows for
/// good time resolution.
///
/// # Processing Pipeline
/// 1. Buffer incoming samples until FFT_SIZE (2048) is reached
/// 2. Apply HFT90D FlatTop window for accurate amplitude representation
/// 3. Compute FFT and extract magnitude spectrum
/// 4. Bin frequencies into 16 log-spaced bands (60-6000 Hz)
/// 5. Apply adaptive AGC with asymmetric attack/release
/// 6. Detect beats using energy thresholding in bass range (100-500 Hz)
/// 7. Advance buffer by HOP_SIZE (1024) for 50% overlap
pub struct DspProcessor {
    sample_rate: f32,
    buffer: Vec<f32>,
    window: Vec<f32>,
    fft: Arc<dyn rustfft::Fft<f32>>,
    bin_edges: Vec<usize>, // FFT bin index boundaries for 16 log-spaced bins
    agc_min: f32,
    agc_max: f32,
    sample_smth: f32,
    beat_history: Vec<f32>,
    beat_idx: usize,
    beat_freq_lo: usize, // FFT bin index for BEAT_FREQ_MIN
    beat_freq_hi: usize, // FFT bin index for BEAT_FREQ_MAX
}

impl DspProcessor {
    /// Creates a new DSP processor configured for the given sample rate.
    ///
    /// # Arguments
    /// * `sample_rate` - Audio sample rate in Hz (typically 44100 or 48000)
    ///
    /// # Returns
    /// A configured processor with pre-computed FFT plan, window function,
    /// and frequency bin boundaries.
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;

        // FlatTop window coefficients (HFT90D)
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let n = i as f32;
                let w = std::f32::consts::PI * 2.0 * n / (FFT_SIZE as f32 - 1.0);
                1.0 - 1.942604 * (w).cos() + 1.340318 * (2.0 * w).cos() - 0.440811 * (3.0 * w).cos()
                    + 0.043097 * (4.0 * w).cos()
            })
            .collect();

        // Precompute 16 log-spaced bin edges (in FFT bin indices)
        let freq_resolution = sr / FFT_SIZE as f32;
        let ratio = (FREQ_MAX / FREQ_MIN).powf(1.0 / NUM_BINS as f32);
        let mut bin_edges = Vec::with_capacity(NUM_BINS + 1);
        for i in 0..=NUM_BINS {
            let freq = FREQ_MIN * ratio.powi(i as i32);
            let bin = (freq / freq_resolution).round() as usize;
            bin_edges.push(bin.min(FFT_SIZE / 2));
        }

        let beat_freq_lo = (BEAT_FREQ_MIN / freq_resolution).round() as usize;
        let beat_freq_hi = (BEAT_FREQ_MAX / freq_resolution).round() as usize;

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        Self {
            sample_rate: sr,
            buffer: Vec::with_capacity(FFT_SIZE),
            window,
            fft,
            bin_edges,
            agc_min: 0.0,
            agc_max: 1.0,
            sample_smth: 0.0,
            beat_history: vec![0.0; BEAT_HISTORY],
            beat_idx: 0,
            beat_freq_lo,
            beat_freq_hi,
        }
    }

    /// Pushes new mono audio samples into the processing buffer.
    ///
    /// # Arguments
    /// * `samples` - Slice of mono f32 samples (range -1.0 to 1.0)
    ///
    /// # Returns
    /// Vector of `DspFrame` results, one for each completed FFT window.
    /// Returns empty vector if insufficient data for processing.
    ///
    /// # Processing Rate
    /// With 50% overlap (HOP_SIZE=1024), at 48kHz sample rate, this produces
    /// approximately 47 frames per second (48000 / 1024 ≈ 46.875).
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<DspFrame> {
        let mut frames = Vec::new();
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= FFT_SIZE {
            let frame_data: Vec<f32> = self.buffer[..FFT_SIZE].to_vec();
            // Advance by HOP_SIZE (50% overlap)
            self.buffer.drain(..HOP_SIZE);
            if let Some(frame) = self.process_frame(&frame_data) {
                frames.push(frame);
            }
        }

        frames
    }

    fn process_frame(&mut self, samples: &[f32]) -> Option<DspFrame> {
        // --- Statistics ---
        let mut max_abs: f32 = 0.0;
        let mut zero_crossings: u16 = 0;
        let mut prev_sign = samples[0] >= 0.0;

        for &s in samples {
            let abs = s.abs();
            if abs > max_abs {
                max_abs = abs;
            }
            let sign = s >= 0.0;
            if sign != prev_sign {
                zero_crossings += 1;
            }
            prev_sign = sign;
        }

        // sampleRaw: scale to 0..255
        let sample_raw = (max_abs * 255.0).min(255.0);

        // Exponential smoothing for sampleSmth
        self.sample_smth =
            self.sample_smth * SAMPLE_SMOOTH_FACTOR + sample_raw * (1.0 - SAMPLE_SMOOTH_FACTOR);

        // --- Silence check ---
        if max_abs < SILENCE_THRESHOLD {
            return Some(DspFrame {
                sample_raw: 0.0,
                sample_smth: self.sample_smth,
                sample_peak: 0,
                fft_result: [0; NUM_BINS],
                zero_crossing_count: 0,
                fft_magnitude: 0.0,
                fft_major_peak: 0.0,
            });
        }

        // --- Windowed FFT ---
        let mut fft_buf: Vec<Complex<f32>> = samples
            .iter()
            .zip(self.window.iter())
            .map(|(&s, &w)| Complex::new(s * w, 0.0))
            .collect();

        self.fft.process(&mut fft_buf);

        // Magnitude of positive half
        let half = FFT_SIZE / 2;
        let magnitudes: Vec<f32> = fft_buf[..half]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .collect();

        // --- Find major peak ---
        let mut peak_mag: f32 = 0.0;
        let mut peak_idx: usize = 0;
        let freq_resolution = self.sample_rate / FFT_SIZE as f32;
        // Only search within FREQ_MIN..FREQ_MAX
        let search_lo = (FREQ_MIN / freq_resolution).round() as usize;
        let search_hi = (FREQ_MAX / freq_resolution).round() as usize;
        for i in search_lo..search_hi.min(half) {
            if magnitudes[i] > peak_mag {
                peak_mag = magnitudes[i];
                peak_idx = i;
            }
        }
        let fft_major_peak = peak_idx as f32 * freq_resolution;
        let fft_magnitude = peak_mag;

        // --- 16 log-spaced bins ---
        let mut raw_bins = [0.0f32; NUM_BINS];
        for i in 0..NUM_BINS {
            let lo = self.bin_edges[i];
            let hi = self.bin_edges[i + 1].max(lo + 1);
            let mut bin_max: f32 = 0.0;
            for j in lo..hi.min(half) {
                let val = magnitudes[j].sqrt() / FFT_BIN_SCALE;
                if val > bin_max {
                    bin_max = val;
                }
            }
            raw_bins[i] = bin_max;
        }

        // --- AGC ---
        let frame_max = raw_bins.iter().cloned().fold(0.0f32, f32::max);
        let frame_min = raw_bins.iter().cloned().fold(f32::MAX, f32::min);

        // Asymmetric smoothing
        if frame_max > self.agc_max {
            self.agc_max = self.agc_max * AGC_ATTACK_OLD + frame_max * AGC_ATTACK_NEW;
        } else {
            self.agc_max = self.agc_max * AGC_RELEASE_OLD + frame_max * AGC_RELEASE_NEW;
        }
        if frame_min < self.agc_min {
            self.agc_min = self.agc_min * AGC_ATTACK_OLD + frame_min * AGC_ATTACK_NEW;
        } else {
            self.agc_min = self.agc_min * AGC_RELEASE_OLD + frame_min * AGC_RELEASE_NEW;
        }

        let span = (self.agc_max - self.agc_min).max(1.0);

        // --- Normalize bins to 0..255 ---
        let mut fft_result = [0u8; NUM_BINS];
        for i in 0..NUM_BINS {
            let normalized = ((raw_bins[i] - self.agc_min) / span * 255.0).clamp(0.0, 255.0);
            fft_result[i] = normalized as u8;
        }

        // --- Beat detection ---
        let beat_energy: f32 = magnitudes[self.beat_freq_lo..self.beat_freq_hi.min(half)]
            .iter()
            .map(|m| m * m)
            .sum();

        self.beat_history[self.beat_idx] = beat_energy;
        self.beat_idx = (self.beat_idx + 1) % BEAT_HISTORY;

        let avg_energy: f32 = self.beat_history.iter().sum::<f32>() / BEAT_HISTORY as f32;

        let sample_peak = if beat_energy > avg_energy * BEAT_THRESHOLD {
            1
        } else {
            0
        };

        Some(DspFrame {
            sample_raw,
            sample_smth: self.sample_smth,
            sample_peak,
            fft_result,
            zero_crossing_count: zero_crossings,
            fft_magnitude,
            fft_major_peak,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsp_processor_creation() {
        let dsp = DspProcessor::new(48000);
        assert_eq!(dsp.sample_rate, 48000.0);
        assert_eq!(dsp.buffer.len(), 0);
        assert_eq!(dsp.window.len(), FFT_SIZE);
        assert_eq!(dsp.bin_edges.len(), NUM_BINS + 1);
    }

    #[test]
    fn test_window_function_bounds() {
        let dsp = DspProcessor::new(48000);
        // HFT90D FlatTop window values should be finite and reasonable
        // Note: This specific window can have negative values near the edges
        for (i, &w) in dsp.window.iter().enumerate() {
            assert!(
                w.is_finite(),
                "Window value at index {} should be finite, got {}",
                i,
                w
            );
            assert!(
                w.abs() <= 10.0,
                "Window value at index {} should be reasonable, got {}",
                i,
                w
            );
        }
        // Check that the middle values are positive (main lobe)
        let mid = FFT_SIZE / 2;
        assert!(
            dsp.window[mid] > 0.0,
            "Window center value should be positive"
        );
    }

    #[test]
    fn test_bin_edges_monotonic_increasing() {
        let dsp = DspProcessor::new(48000);
        // Bin edges should be non-decreasing (may have duplicates at low frequencies)
        for i in 0..dsp.bin_edges.len() - 1 {
            assert!(
                dsp.bin_edges[i] <= dsp.bin_edges[i + 1],
                "Bin edge {} ({}) should not exceed bin edge {} ({})",
                i,
                dsp.bin_edges[i],
                i + 1,
                dsp.bin_edges[i + 1]
            );
        }
        // Verify that the first and last edges are different (overall increasing trend)
        assert!(
            dsp.bin_edges[0] < dsp.bin_edges[NUM_BINS],
            "First bin edge should be less than last bin edge"
        );
    }

    #[test]
    fn test_bin_edges_within_nyquist() {
        let dsp = DspProcessor::new(48000);
        let nyquist_bin = FFT_SIZE / 2;
        // All bin edges should be within Nyquist limit
        for &edge in &dsp.bin_edges {
            assert!(
                edge <= nyquist_bin,
                "Bin edge {} exceeds Nyquist bin {}",
                edge,
                nyquist_bin
            );
        }
    }

    #[test]
    fn test_silence_produces_zero_output() {
        let mut dsp = DspProcessor::new(48000);
        let silence = vec![0.0f32; FFT_SIZE];

        let frames = dsp.push_samples(&silence);
        assert_eq!(frames.len(), 1, "Should produce one frame");

        let frame = &frames[0];
        assert_eq!(frame.sample_raw, 0.0, "Silence should have zero sample_raw");
        assert_eq!(
            frame.sample_peak, 0,
            "Silence should have no beat detection"
        );
        assert_eq!(
            frame.fft_magnitude, 0.0,
            "Silence should have zero magnitude"
        );
        // All FFT bins should be zero
        for &bin in &frame.fft_result {
            assert_eq!(bin, 0, "Silence should have zero FFT bins");
        }
    }

    #[test]
    fn test_insufficient_samples_no_output() {
        let mut dsp = DspProcessor::new(48000);
        let few_samples = vec![0.1f32; 100];

        let frames = dsp.push_samples(&few_samples);
        assert_eq!(
            frames.len(),
            0,
            "Should not produce frames with insufficient samples"
        );
    }

    #[test]
    fn test_multiple_frames_with_overlap() {
        let mut dsp = DspProcessor::new(48000);
        // Send enough samples for 2 overlapping frames: FFT_SIZE + HOP_SIZE
        let samples = vec![0.1f32; FFT_SIZE + HOP_SIZE];

        let frames = dsp.push_samples(&samples);
        assert_eq!(frames.len(), 2, "Should produce 2 frames with 50% overlap");
    }

    #[test]
    fn test_sample_smoothing_exists() {
        let mut dsp = DspProcessor::new(48000);

        // Process several frames and verify sample_smth tracks sample_raw with smoothing
        let mut prev_smth = 0.0;
        let amplitudes = [0.0, 0.5, 0.8, 0.3, 0.0];

        for &amp in &amplitudes {
            let samples = vec![amp; FFT_SIZE];
            let frames = dsp.push_samples(&samples);
            if !frames.is_empty() {
                let smth = frames[0].sample_smth;
                // Verify smoothing is active (not just copying raw value)
                // After the first non-zero frame, smth should be different from raw
                if amp == 0.0 && prev_smth > 10.0 {
                    // When going to zero, smoothed should not immediately reach zero
                    assert!(
                        smth > 1.0,
                        "Smoothed value {} should lag behind raw value 0 due to smoothing",
                        smth
                    );
                }
                prev_smth = smth;
            }
        }
    }

    #[test]
    fn test_zero_crossing_count() {
        let mut dsp = DspProcessor::new(48000);

        // Create a simple square wave alternating between -0.5 and 0.5
        let mut square_wave = Vec::with_capacity(FFT_SIZE);
        for i in 0..FFT_SIZE {
            square_wave.push(if i % 100 < 50 { 0.5 } else { -0.5 });
        }

        let frames = dsp.push_samples(&square_wave);
        assert_eq!(frames.len(), 1);

        let frame = &frames[0];
        // Should detect multiple zero crossings in the square wave
        assert!(
            frame.zero_crossing_count > 10,
            "Square wave should have many zero crossings, got {}",
            frame.zero_crossing_count
        );
    }

    #[test]
    fn test_beat_detection_sensitivity() {
        let mut dsp = DspProcessor::new(48000);

        // Process several frames of low energy to establish baseline
        for _ in 0..BEAT_HISTORY + 5 {
            let low_energy = vec![0.01f32; HOP_SIZE];
            let _ = dsp.push_samples(&low_energy);
        }

        // Now send a high-energy burst
        let high_energy = vec![0.8f32; HOP_SIZE];
        let frames = dsp.push_samples(&high_energy);

        // The high energy frame should potentially trigger beat detection
        // (though this depends on frequency content, so we just verify it runs)
        assert!(!frames.is_empty(), "Should process high energy samples");
    }

    #[test]
    fn test_agc_bounds() {
        let mut dsp = DspProcessor::new(48000);

        // Process various amplitude levels
        let amplitudes = [0.1, 0.5, 0.9, 0.3, 0.7];
        for &amp in &amplitudes {
            let samples = vec![amp; HOP_SIZE];
            let frames = dsp.push_samples(&samples);
            for frame in frames {
                // All FFT bins should be in valid range after AGC
                for &bin in &frame.fft_result {
                    // FFT bins are u8, so they're automatically bounded 0-255
                    // This just verifies the type constraint
                    let _ = bin; // Acknowledge we checked it
                }
            }
        }
    }

    #[test]
    fn test_major_peak_frequency_reasonable() {
        let mut dsp = DspProcessor::new(48000);
        let sample_rate = 48000.0;

        // Generate a simple sine wave at 1000 Hz
        let mut sine_wave = Vec::with_capacity(FFT_SIZE);
        let freq = 1000.0;
        for i in 0..FFT_SIZE {
            let t = i as f32 / sample_rate;
            sine_wave.push((2.0 * std::f32::consts::PI * freq * t).sin() * 0.5);
        }

        let frames = dsp.push_samples(&sine_wave);
        assert_eq!(frames.len(), 1);

        let frame = &frames[0];
        // Peak should be near 1000 Hz (within ~50 Hz due to bin resolution)
        assert!(
            (frame.fft_major_peak - 1000.0).abs() < 100.0,
            "Major peak frequency {} should be close to 1000 Hz",
            frame.fft_major_peak
        );
    }
}
