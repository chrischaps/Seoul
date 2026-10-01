use std::thread;
use std::time::{Duration, Instant};

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use ringbuf::traits::Consumer as _;
use tracing::info;
use triple_buffer::{Input, Output, TripleBuffer};

use crate::audio::features::{AudioFeatures, SPECTRUM_BINS, WAVEFORM_SAMPLES};

const FFT_SIZE: usize = 2048;
const MIN_NEW_SAMPLES: usize = 256;
const PEAK_DECAY: f32 = 0.9995;
const SMOOTHING: f32 = 0.35;
const BEAT_HISTORY: usize = 64;
const BEAT_THRESHOLD_RATIO: f32 = 1.4;
const BEAT_MIN_ENERGY: f32 = 0.12;
const BEAT_DECAY: f32 = 0.85;

pub fn spawn_analysis(
    consumer: ringbuf::HeapCons<f32>,
    sample_rate: u32,
) -> Output<AudioFeatures> {
    let (input, output) = TripleBuffer::new(&AudioFeatures::default()).split();

    thread::Builder::new()
        .name("seoul-analysis".into())
        .spawn(move || analysis_loop(consumer, sample_rate, input))
        .expect("spawn analysis thread");

    output
}

fn analysis_loop(
    mut consumer: ringbuf::HeapCons<f32>,
    sample_rate: u32,
    mut out: Input<AudioFeatures>,
) {
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut fft_in: Vec<f32> = fft.make_input_vec();
    let mut fft_out: Vec<Complex<f32>> = fft.make_output_vec();
    let mut scratch: Vec<f32> = vec![0.0; FFT_SIZE];
    let mut mags: Vec<f32> = vec![0.0; FFT_SIZE / 2 + 1];

    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

    let mut rolling: Vec<f32> = vec![0.0; FFT_SIZE];
    let mut write_pos: usize = 0;
    let mut features = AudioFeatures::default();
    let mut running_peak = 1e-6f32;
    let mut beat_history = vec![0.0f32; BEAT_HISTORY];
    let mut beat_idx: usize = 0;

    let spectrum_map = build_log_bin_map(sample_rate);
    let (bass_range, mid_range, treble_range) = band_ranges(sample_rate);

    info!(
        fft_size = FFT_SIZE,
        sample_rate,
        bass = ?bass_range,
        mid = ?mid_range,
        treble = ?treble_range,
        "analysis thread ready"
    );

    let start = Instant::now();
    let mut accumulated_new = 0usize;

    loop {
        let mut pulled = 0usize;
        while let Some(s) = consumer.try_pop() {
            rolling[write_pos] = s;
            write_pos = (write_pos + 1) % FFT_SIZE;
            pulled += 1;
        }
        accumulated_new = accumulated_new.saturating_add(pulled);

        if accumulated_new < MIN_NEW_SAMPLES {
            thread::sleep(Duration::from_millis(4));
            continue;
        }
        accumulated_new = 0;

        // Unroll rolling buffer into scratch (oldest → newest)
        for i in 0..FFT_SIZE {
            scratch[i] = rolling[(write_pos + i) % FFT_SIZE];
        }

        // Waveform: last WAVEFORM_SAMPLES (pre-window, raw)
        let wf_start = FFT_SIZE - WAVEFORM_SAMPLES;
        features.waveform.copy_from_slice(&scratch[wf_start..]);

        // RMS volume on raw scratch
        let sum_sq: f32 = scratch.iter().map(|s| s * s).sum();
        let rms = (sum_sq / FFT_SIZE as f32).sqrt();

        // Window + FFT
        for i in 0..FFT_SIZE {
            fft_in[i] = scratch[i] * window[i];
        }
        fft.process(&mut fft_in, &mut fft_out).expect("FFT");
        for (m, c) in mags.iter_mut().zip(fft_out.iter()) {
            *m = c.norm();
        }

        // Running peak for normalization — divide and conquer loudness
        let frame_peak = mags.iter().copied().fold(0.0f32, f32::max);
        running_peak = (running_peak * PEAK_DECAY).max(frame_peak).max(1e-6);
        let inv_peak = 1.0 / running_peak;

        // Log-spaced spectrum
        for (i, (lo, hi)) in spectrum_map.iter().enumerate() {
            let count = (*hi - *lo).max(1);
            let sum: f32 = mags[*lo..*hi].iter().sum();
            features.spectrum[i] = (sum / count as f32 * inv_peak).min(1.0);
        }

        // Band energies (raw, normalized, clamped)
        let bass_raw = band_energy(&mags, bass_range) * inv_peak;
        let mid_raw = band_energy(&mags, mid_range) * inv_peak;
        let treble_raw = band_energy(&mags, treble_range) * inv_peak;

        features.bass = lerp(features.bass, bass_raw.min(1.0), SMOOTHING);
        features.mid = lerp(features.mid, mid_raw.min(1.0), SMOOTHING);
        features.treble = lerp(features.treble, treble_raw.min(1.0), SMOOTHING);
        features.volume = lerp(features.volume, rms.min(1.0), SMOOTHING);

        // Beat detection from smoothed bass
        let avg: f32 = beat_history.iter().sum::<f32>() / beat_history.len() as f32;
        if features.bass > BEAT_MIN_ENERGY && features.bass > avg * BEAT_THRESHOLD_RATIO {
            features.beat = 1.0;
        } else {
            features.beat *= BEAT_DECAY;
        }
        beat_history[beat_idx] = features.bass;
        beat_idx = (beat_idx + 1) % beat_history.len();

        features.time = start.elapsed().as_secs_f32();

        out.write(features);
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn band_energy(mags: &[f32], range: (usize, usize)) -> f32 {
    let (lo, hi) = range;
    if hi <= lo {
        return 0.0;
    }
    let sum: f32 = mags[lo..hi].iter().sum();
    sum / (hi - lo) as f32
}

fn band_ranges(sample_rate: u32) -> ((usize, usize), (usize, usize), (usize, usize)) {
    let hz_per_bin = sample_rate as f32 / FFT_SIZE as f32;
    let nyquist_bin = FFT_SIZE / 2;
    let hz_to_bin = |hz: f32| ((hz / hz_per_bin) as usize).min(nyquist_bin);
    let bass = (hz_to_bin(60.0).max(1), hz_to_bin(250.0));
    let mid = (bass.1, hz_to_bin(4000.0));
    let treble = (mid.1, nyquist_bin + 1);
    (bass, mid, treble)
}

fn build_log_bin_map(sample_rate: u32) -> Vec<(usize, usize)> {
    let hz_per_bin = sample_rate as f32 / FFT_SIZE as f32;
    let f_min = 50.0f32;
    let f_max = (sample_rate as f32 / 2.0).min(20_000.0);
    let log_min = f_min.ln();
    let log_max = f_max.ln();
    let nyquist_bin = FFT_SIZE / 2;

    let mut map = Vec::with_capacity(SPECTRUM_BINS);
    let mut prev_bin = ((f_min / hz_per_bin) as usize).max(1);
    for i in 0..SPECTRUM_BINS {
        let t = (i + 1) as f32 / SPECTRUM_BINS as f32;
        let freq = (log_min + (log_max - log_min) * t).exp();
        let bin = ((freq / hz_per_bin) as usize).min(nyquist_bin);
        let lo = prev_bin;
        let hi = bin.max(lo + 1);
        map.push((lo, hi));
        prev_bin = hi;
    }
    map
}
