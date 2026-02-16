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

/// Output of DSP processing for one frame.
pub struct DspFrame {
    pub sample_raw: f32,
    pub sample_smth: f32,
    pub sample_peak: u8,
    pub fft_result: [u8; NUM_BINS],
    pub zero_crossing_count: u16,
    pub fft_magnitude: f32,
    pub fft_major_peak: f32,
}

pub struct DspProcessor {
    sample_rate: f32,
    buffer: Vec<f32>,
    window: Vec<f32>,
    fft: Arc<dyn rustfft::Fft<f32>>,
    bin_edges: Vec<usize>,     // FFT bin index boundaries for 16 log-spaced bins
    agc_min: f32,
    agc_max: f32,
    sample_smth: f32,
    beat_history: Vec<f32>,
    beat_idx: usize,
    beat_freq_lo: usize,       // FFT bin index for BEAT_FREQ_MIN
    beat_freq_hi: usize,       // FFT bin index for BEAT_FREQ_MAX
}

impl DspProcessor {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;

        // FlatTop window coefficients (HFT90D)
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let n = i as f32;
                let w = std::f32::consts::PI * 2.0 * n / (FFT_SIZE as f32 - 1.0);
                1.0 - 1.942604 * (w).cos()
                    + 1.340318 * (2.0 * w).cos()
                    - 0.440811 * (3.0 * w).cos()
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

    /// Push new mono samples. Returns a DspFrame each time a hop-worth of data is ready.
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
        self.sample_smth = self.sample_smth * 0.7 + sample_raw * 0.3;

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
                let val = magnitudes[j].sqrt() / 0.04194;
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
        let beat_energy: f32 = magnitudes
            [self.beat_freq_lo..self.beat_freq_hi.min(half)]
            .iter()
            .map(|m| m * m)
            .sum();

        self.beat_history[self.beat_idx] = beat_energy;
        self.beat_idx = (self.beat_idx + 1) % BEAT_HISTORY;

        let avg_energy: f32 =
            self.beat_history.iter().sum::<f32>() / BEAT_HISTORY as f32;

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
