//! Audio feature extraction.
//!
//! [`Analyzer`] is a pure, deterministic DSP core: it consumes mono samples
//! and advances on a fixed hop of the *sample* clock, so every time constant
//! below is in seconds of audio rather than "per tick". The analysis thread
//! wraps it with I/O concerns — draining the capture ring, feeding synthetic
//! silence when WASAPI loopback stops delivering packets, and swapping in a
//! new source when capture reconnects.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use ringbuf::traits::Consumer as _;
use tracing::info;
use triple_buffer::{Input, Output, TripleBuffer};

use crate::audio::features::{AudioFeatures, SPECTRUM_BINS, WAVEFORM_SAMPLES};

const FFT_SIZE: usize = 2048;
/// Analysis hop in samples (~5.8 ms at 44.1 kHz).
const HOP: usize = 256;

/// Absolute floor below which everything reads as silence (dBFS-ish, where a
/// full-scale sine maps to 0 dB). Keeps the AGC from amplifying noise.
const FLOOR_DB: f32 = -50.0;

// Band envelopes.
const BAND_ATTACK: f32 = 0.012;
const BAND_RELEASE: f32 = 0.150;
const BAND_REF_RELEASE: f32 = 3.0;
const ATT_TAU: f32 = 1.0;

// Spectrum display.
const SPEC_RANGE_DB: f32 = 48.0;
const SPEC_REF_FLOOR_DB: f32 = -35.0;
const SPEC_BOTTOM_DB: f32 = -75.0;
const SPEC_REF_RELEASE_DB_PER_S: f32 = 6.0;
const SPEC_TILT_DB_PER_OCT: f32 = 3.0;
const SPEC_RISE: f32 = 0.010;
const SPEC_FALL: f32 = 0.120;

// Onset / beat detection.
const ONSET_HISTORY_S: f32 = 1.0;
const ONSET_K: f32 = 1.5;
const ONSET_MIN: f32 = 0.02;
const BEAT_REFRACTORY: f32 = 0.18;
const BEAT_TAU: f32 = 0.10;

// Tempo estimation.
const TEMPO_WINDOW_S: f32 = 10.0;
const BPM_MIN: f32 = 80.0;
const BPM_MAX: f32 = 160.0;
const BPM_DEFAULT: f32 = 120.0;

// Waveform.
const WAVE_SEARCH: usize = 512;
const WAVE_TAPER: usize = 24;
const WAVE_REF_RELEASE: f32 = 2.0;

/// A capture stream's sample ring + its rate. Sent to the analysis thread
/// whenever capture (re)connects.
pub struct AudioSource {
    pub consumer: ringbuf::HeapCons<f32>,
    pub sample_rate: u32,
}

/// Spawn the analysis thread. Returns the features reader for the render
/// thread and a sender for (re)attaching audio sources.
pub fn spawn_analysis() -> (Output<AudioFeatures>, Sender<AudioSource>) {
    let (input, output) = TripleBuffer::new(&AudioFeatures::default()).split();
    let (tx, rx) = channel();

    thread::Builder::new()
        .name("seoul-analysis".into())
        .spawn(move || analysis_loop(rx, input))
        .expect("spawn analysis thread");

    (output, tx)
}

fn analysis_loop(sources: Receiver<AudioSource>, mut out: Input<AudioFeatures>) {
    // Until a source arrives we run at a nominal rate on synthesized silence.
    let mut source: Option<ringbuf::HeapCons<f32>> = None;
    let mut analyzer = Analyzer::new(48_000);
    let mut buf: Vec<f32> = Vec::with_capacity(8192);
    let zeros = vec![0.0f32; 4096];

    let mut last_real = Instant::now();
    let mut silence_clock = Instant::now();

    loop {
        // Latest source wins; a sample-rate change rebuilds the analyzer.
        while let Ok(src) = sources.try_recv() {
            if src.sample_rate as f32 != analyzer.sample_rate {
                analyzer = Analyzer::new(src.sample_rate);
            }
            source = Some(src.consumer);
            info!(sample_rate = src.sample_rate, "analysis attached to audio source");
        }

        buf.clear();
        if let Some(c) = source.as_mut() {
            buf.extend(c.pop_iter());
        }

        let now = Instant::now();
        let mut published = false;
        if !buf.is_empty() {
            published = analyzer.push(&buf);
            last_real = now;
            silence_clock = now;
        } else if now.duration_since(last_real) > Duration::from_millis(40) {
            // WASAPI loopback sends nothing while the system is silent. Feed
            // real-time zeros so every envelope decays exactly as it would on
            // genuine digital silence.
            let owed = (now.duration_since(silence_clock).as_secs_f32()
                * analyzer.sample_rate) as usize;
            if owed >= HOP {
                let n = owed.min(zeros.len());
                published = analyzer.push(&zeros[..n]);
                silence_clock = now;
            }
        }

        if published {
            out.write(*analyzer.features());
        }
        thread::sleep(Duration::from_millis(3));
    }
}

pub struct Analyzer {
    sample_rate: f32,
    hop_dt: f32,

    fft: Arc<dyn RealToComplex<f32>>,
    fft_in: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    mags: Vec<f32>,
    log_mags: Vec<f32>,
    prev_log_mags: Vec<f32>,

    rolling: Vec<f32>,
    write_pos: usize,
    pending: usize,
    frame: Vec<f32>,

    spectrum_map: Vec<(usize, usize)>,
    spectrum_tilt_db: Vec<f32>,
    bands: [(usize, usize); 3],
    kick_range: (usize, usize),
    flux_hi_bin: usize,

    band_ref: [f32; 3],
    band_val: [f32; 3],
    band_att: [f32; 3],
    vol_ref: f32,
    volume: f32,
    spec_ref_db: f32,
    wave_ref: f32,

    onset_hist: VecDeque<f32>,
    onset_hist_len: usize,
    onset_prev: [f32; 2],
    since_beat: f32,
    beat: f32,
    beat_count: u32,

    clock: f32,
    onset_times: VecDeque<f32>,
    bpm: f32,
    bpm_confidence: f32,
    phase: f32,

    features: AudioFeatures,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let fft_in = fft.make_input_vec();
        let fft_out = fft.make_output_vec();
        let fft_scratch = fft.make_scratch_vec();
        let n_bins = FFT_SIZE / 2 + 1;

        let window = (0..FFT_SIZE)
            .map(|i| 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos()))
            .collect();

        let spectrum_map = build_log_bin_map(sr);
        let hz_per_bin = sr / FFT_SIZE as f32;
        let spectrum_tilt_db = spectrum_map
            .iter()
            .map(|&(lo, hi)| {
                let fc = (lo + hi) as f32 * 0.5 * hz_per_bin;
                SPEC_TILT_DB_PER_OCT * (fc.max(20.0) / 1000.0).log2()
            })
            .collect();

        let hz = |f: f32| ((f / hz_per_bin) as usize).clamp(1, n_bins - 1);
        let bands = [
            (hz(40.0), hz(250.0)),
            (hz(250.0), hz(4000.0)),
            (hz(4000.0), hz(16000.0)),
        ];

        info!(
            fft_size = FFT_SIZE,
            hop = HOP,
            sample_rate,
            bass = ?bands[0],
            mid = ?bands[1],
            treble = ?bands[2],
            "analyzer ready"
        );

        let hop_dt = HOP as f32 / sr;
        let onset_hist_len = (ONSET_HISTORY_S / hop_dt) as usize;

        Self {
            sample_rate: sr,
            hop_dt,
            fft,
            fft_in,
            fft_out,
            fft_scratch,
            window,
            mags: vec![0.0; n_bins],
            log_mags: vec![0.0; n_bins],
            prev_log_mags: vec![0.0; n_bins],
            rolling: vec![0.0; FFT_SIZE],
            write_pos: 0,
            pending: 0,
            frame: vec![0.0; FFT_SIZE],
            spectrum_map,
            spectrum_tilt_db,
            bands,
            kick_range: (hz(30.0), hz(180.0)),
            flux_hi_bin: hz(8000.0),
            band_ref: [db_to_amp(FLOOR_DB); 3],
            band_val: [0.0; 3],
            band_att: [0.0; 3],
            vol_ref: db_to_amp(FLOOR_DB),
            volume: 0.0,
            spec_ref_db: SPEC_REF_FLOOR_DB,
            wave_ref: db_to_amp(FLOOR_DB),
            onset_hist: VecDeque::with_capacity(onset_hist_len + 1),
            onset_hist_len,
            onset_prev: [0.0; 2],
            since_beat: f32::INFINITY,
            beat: 0.0,
            beat_count: 0,
            clock: 0.0,
            onset_times: VecDeque::new(),
            bpm: BPM_DEFAULT,
            bpm_confidence: 0.0,
            phase: 0.0,
            features: AudioFeatures::default(),
        }
    }

    pub fn features(&self) -> &AudioFeatures {
        &self.features
    }

    /// Feed mono samples. Returns true if at least one hop was analyzed
    /// (i.e. `features()` changed).
    pub fn push(&mut self, samples: &[f32]) -> bool {
        let mut ran = false;
        for &s in samples {
            self.rolling[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % FFT_SIZE;
            self.pending += 1;
            if self.pending >= HOP {
                self.pending = 0;
                self.hop();
                ran = true;
            }
        }
        ran
    }

    fn hop(&mut self) {
        let dt = self.hop_dt;
        self.clock += dt;

        // Unroll the ring, oldest → newest.
        for i in 0..FFT_SIZE {
            self.frame[i] = self.rolling[(self.write_pos + i) % FFT_SIZE];
        }

        // Spectrum via windowed real FFT. Normalize so a full-scale sine
        // peaks at ~1.0 (Hann coherent gain 0.5 × N/2).
        for i in 0..FFT_SIZE {
            self.fft_in[i] = self.frame[i] * self.window[i];
        }
        self.fft
            .process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.fft_scratch)
            .expect("FFT buffer sizes are fixed at construction");
        let norm = 4.0 / FFT_SIZE as f32;
        for (m, c) in self.mags.iter_mut().zip(self.fft_out.iter()) {
            *m = c.norm() * norm;
        }

        self.update_bands(dt);
        self.update_spectrum(dt);
        self.update_beat(dt);
        self.update_tempo(dt);
        self.update_waveform(dt);

        let f = &mut self.features;
        f.bass = self.band_val[0];
        f.mid = self.band_val[1];
        f.treble = self.band_val[2];
        f.bass_att = self.band_att[0];
        f.mid_att = self.band_att[1];
        f.treble_att = self.band_att[2];
        f.volume = self.volume;
        f.beat = self.beat;
        f.beat_count = self.beat_count as f32;
        f.bpm = self.bpm;
        f.bpm_confidence = self.bpm_confidence;
        f.beat_phase = self.phase;
    }

    fn update_bands(&mut self, dt: f32) {
        let floor = db_to_amp(FLOOR_DB);
        let ref_decay = (-dt / BAND_REF_RELEASE).exp();
        for b in 0..3 {
            let (lo, hi) = self.bands[b];
            // Band energy, not mean: wide bands aren't diluted by bin count.
            let raw = self.mags[lo..hi].iter().map(|m| m * m).sum::<f32>().sqrt();
            // Per-band AGC: the reference tracks recent peaks and relaxes
            // toward the floor, so each band uses its full 0..1 range.
            self.band_ref[b] = raw.max(floor + (self.band_ref[b] - floor) * ref_decay);
            let target = if raw > floor { raw / self.band_ref[b] } else { 0.0 };
            self.band_val[b] = follow(self.band_val[b], target, dt, BAND_ATTACK, BAND_RELEASE);
            self.band_att[b] = follow(self.band_att[b], self.band_val[b], dt, ATT_TAU, ATT_TAU);
        }

        let rms = (self.frame.iter().map(|s| s * s).sum::<f32>() / FFT_SIZE as f32).sqrt();
        // Sine RMS is 0.707 of peak; scale so full-scale ≈ 0 dB like the bands.
        let rms = rms * std::f32::consts::SQRT_2;
        self.vol_ref = rms.max(floor + (self.vol_ref - floor) * ref_decay);
        let target = if rms > floor { rms / self.vol_ref } else { 0.0 };
        self.volume = follow(self.volume, target, dt, BAND_ATTACK, BAND_RELEASE);
    }

    fn update_spectrum(&mut self, dt: f32) {
        let mut frame_max_db = f32::NEG_INFINITY;
        let mut dbs = [0.0f32; SPECTRUM_BINS];
        for (i, &(lo, hi)) in self.spectrum_map.iter().enumerate() {
            let count = (hi - lo).max(1) as f32;
            let power = self.mags[lo..hi].iter().map(|m| m * m).sum::<f32>() / count;
            let db = 10.0 * (power + 1e-12).log10() + self.spectrum_tilt_db[i];
            dbs[i] = db;
            frame_max_db = frame_max_db.max(db);
        }

        self.spec_ref_db = frame_max_db
            .max(self.spec_ref_db - SPEC_REF_RELEASE_DB_PER_S * dt)
            .max(SPEC_REF_FLOOR_DB);
        let top = self.spec_ref_db;
        let bottom = (top - SPEC_RANGE_DB).max(SPEC_BOTTOM_DB);
        let span = (top - bottom).max(1.0);

        for (out, db) in self.features.spectrum.iter_mut().zip(dbs) {
            let target = ((db - bottom) / span).clamp(0.0, 1.0);
            *out = follow(*out, target, dt, SPEC_RISE, SPEC_FALL);
        }
    }

    fn update_beat(&mut self, dt: f32) {
        // Log-compressed magnitudes make flux roughly gain-invariant.
        for (l, &m) in self.log_mags.iter_mut().zip(self.mags.iter()) {
            *l = (1.0 + 1000.0 * m).ln();
        }
        let flux = |range: (usize, usize), cur: &[f32], prev: &[f32]| -> f32 {
            let (lo, hi) = range;
            let sum: f32 = cur[lo..hi]
                .iter()
                .zip(&prev[lo..hi])
                .map(|(c, p)| (c - p).max(0.0))
                .sum();
            sum / (hi - lo).max(1) as f32
        };
        let kick = flux(self.kick_range, &self.log_mags, &self.prev_log_mags);
        let broad = flux((1, self.flux_hi_bin), &self.log_mags, &self.prev_log_mags);
        std::mem::swap(&mut self.log_mags, &mut self.prev_log_mags);
        let onset = kick + 0.5 * broad;

        // Adaptive threshold from the last ~1 s of onset strength.
        let n = self.onset_hist.len().max(1) as f32;
        let mean = self.onset_hist.iter().sum::<f32>() / n;
        let var = self.onset_hist.iter().map(|o| (o - mean).powi(2)).sum::<f32>() / n;
        let threshold = mean + ONSET_K * var.sqrt() + ONSET_MIN;

        // Peak-pick one hop late: the previous value must be a local maximum.
        let [prev2, prev1] = self.onset_prev;
        self.since_beat += dt;
        let is_peak = prev1 > prev2 && prev1 >= onset && prev1 > threshold;
        if is_peak && self.since_beat > BEAT_REFRACTORY {
            self.since_beat = 0.0;
            self.beat = 1.0;
            self.beat_count += 1;
            self.on_beat();
        } else {
            self.beat *= (-dt / BEAT_TAU).exp();
        }

        self.onset_prev = [prev1, onset];
        self.onset_hist.push_back(onset);
        if self.onset_hist.len() > self.onset_hist_len {
            self.onset_hist.pop_front();
        }
    }

    fn on_beat(&mut self) {
        let now = self.clock;
        self.onset_times.push_back(now);
        while self.onset_times.front().is_some_and(|&t| now - t > TEMPO_WINDOW_S) {
            self.onset_times.pop_front();
        }

        // Nudge the phase oscillator toward 0 when a beat lands near it.
        let err = if self.phase > 0.5 { self.phase - 1.0 } else { self.phase };
        if err.abs() < 0.3 {
            self.phase = (self.phase - err * 0.3).rem_euclid(1.0);
        }

        if let Some((bpm, conf)) = estimate_tempo(&self.onset_times) {
            self.bpm_confidence = conf;
            if conf > 0.2 {
                // Snap on big confident jumps (new song), glide otherwise.
                let k = if (bpm - self.bpm).abs() > 8.0 && conf > 0.4 { 1.0 } else { 0.25 };
                self.bpm += (bpm - self.bpm) * k;
            }
        }
    }

    fn update_tempo(&mut self, dt: f32) {
        self.phase = (self.phase + dt * self.bpm / 60.0).rem_euclid(1.0);
        // Confidence fades if beats stop arriving.
        if self.since_beat > 2.0 {
            self.bpm_confidence *= (-dt / 2.0).exp();
        }
    }

    fn update_waveform(&mut self, dt: f32) {
        // Trigger on the steepest rising zero crossing in the search region
        // so periodic material holds still on screen instead of scrolling.
        let end_start = FFT_SIZE - WAVEFORM_SAMPLES;
        let search_lo = end_start.saturating_sub(WAVE_SEARCH).max(2);
        let mut start = end_start;
        let mut best_slope = 0.0f32;
        for i in search_lo..=end_start {
            let (a, b) = (self.frame[i - 1], self.frame[i]);
            if a < 0.0 && b >= 0.0 {
                let slope = self.frame[(i + 2).min(FFT_SIZE - 1)] - self.frame[i - 2];
                if slope > best_slope {
                    best_slope = slope;
                    start = i;
                }
            }
        }
        let seg = &self.frame[start..start + WAVEFORM_SAMPLES];

        let floor = db_to_amp(FLOOR_DB);
        let peak = seg.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let decay = (-dt / WAVE_REF_RELEASE).exp();
        self.wave_ref = peak.max(floor + (self.wave_ref - floor) * decay);
        let gain = if peak > floor * 0.1 { 0.9 / self.wave_ref } else { 0.0 };

        for (i, (out, &s)) in self.features.waveform.iter_mut().zip(seg).enumerate() {
            let edge = i.min(WAVEFORM_SAMPLES - 1 - i);
            let taper = if edge < WAVE_TAPER {
                let x = edge as f32 / WAVE_TAPER as f32;
                (x * std::f32::consts::FRAC_PI_2).sin().powi(2)
            } else {
                1.0
            };
            let target = (s * gain).clamp(-1.0, 1.0) * taper;
            *out += (target - *out) * 0.6;
        }
    }
}

/// Inter-onset-interval histogram tempo estimate, folded into BPM_MIN..BPM_MAX.
/// Returns (bpm, confidence 0..1) once enough onsets are available.
fn estimate_tempo(onsets: &VecDeque<f32>) -> Option<(f32, f32)> {
    if onsets.len() < 6 {
        return None;
    }
    const BINS: usize = (BPM_MAX - BPM_MIN) as usize;
    let mut hist = [0.0f32; BINS];
    let times: Vec<f32> = onsets.iter().copied().collect();
    for i in 0..times.len() {
        for j in (i + 1)..times.len() {
            let d = times[j] - times[i];
            if d > 2.0 {
                break;
            }
            if d < 0.25 {
                continue;
            }
            let mut bpm = 60.0 / d;
            while bpm < BPM_MIN {
                bpm *= 2.0;
            }
            while bpm >= BPM_MAX {
                bpm /= 2.0;
            }
            // Adjacent onsets carry the most tempo information.
            let w = 1.0 / (j - i) as f32;
            for (k, h) in hist.iter_mut().enumerate() {
                let c = BPM_MIN + k as f32 + 0.5;
                let z = (c - bpm) / 1.5;
                *h += w * (-0.5 * z * z).exp();
            }
        }
    }
    let total: f32 = hist.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let (peak, _) = hist
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    // Parabolic refinement around the peak bin.
    let y0 = hist[peak.saturating_sub(1)];
    let y1 = hist[peak];
    let y2 = hist[(peak + 1).min(BINS - 1)];
    let denom = y0 - 2.0 * y1 + y2;
    let offset = if denom.abs() > 1e-6 { 0.5 * (y0 - y2) / denom } else { 0.0 };
    let bpm = BPM_MIN + peak as f32 + 0.5 + offset.clamp(-0.5, 0.5);

    let lo = peak.saturating_sub(3);
    let hi = (peak + 4).min(BINS);
    let near: f32 = hist[lo..hi].iter().sum();
    Some((bpm, (near / total).clamp(0.0, 1.0)))
}

/// One-pole follower with separate rise/fall time constants (seconds).
#[inline]
fn follow(cur: f32, target: f32, dt: f32, rise: f32, fall: f32) -> f32 {
    let tau = if target > cur { rise } else { fall };
    cur + (target - cur) * (1.0 - (-dt / tau).exp())
}

#[inline]
fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn build_log_bin_map(sample_rate: f32) -> Vec<(usize, usize)> {
    let hz_per_bin = sample_rate / FFT_SIZE as f32;
    let f_min = 40.0f32;
    let f_max = (sample_rate / 2.0).min(16_000.0);
    let (log_min, log_max) = (f_min.ln(), f_max.ln());
    let nyquist_bin = FFT_SIZE / 2;

    let mut map = Vec::with_capacity(SPECTRUM_BINS);
    let mut prev_bin = ((f_min / hz_per_bin) as usize).max(1);
    for i in 0..SPECTRUM_BINS {
        let t = (i + 1) as f32 / SPECTRUM_BINS as f32;
        let freq = (log_min + (log_max - log_min) * t).exp();
        let bin = ((freq / hz_per_bin) as usize).min(nyquist_bin);
        let lo = prev_bin.min(nyquist_bin - 1);
        let hi = bin.max(lo + 1);
        map.push((lo, hi));
        prev_bin = hi;
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// Run `seconds` of a generated signal through a fresh analyzer, calling
    /// `each` after every hop.
    fn run(
        seconds: f32,
        mut signal: impl FnMut(f32) -> f32,
        mut each: impl FnMut(&AudioFeatures),
    ) -> Analyzer {
        let mut a = Analyzer::new(SR);
        let n = (seconds * SR as f32) as usize;
        let mut chunk = Vec::with_capacity(HOP);
        for i in 0..n {
            chunk.push(signal(i as f32 / SR as f32));
            if chunk.len() == HOP {
                a.push(&chunk);
                chunk.clear();
                each(a.features());
            }
        }
        a
    }

    /// Deterministic four-on-the-floor at `bpm`: pitched-down kick + soft pad.
    fn kick_at(bpm: f32, t: f32) -> f32 {
        let beat_t = t % (60.0 / bpm);
        let kick = (-beat_t * 18.0).exp()
            * (std::f32::consts::TAU * (50.0 + 90.0 * (-beat_t * 30.0).exp()) * beat_t).sin();
        let pad = 0.05 * (std::f32::consts::TAU * 330.0 * t).sin();
        0.8 * kick + pad
    }

    fn kick_track(t: f32) -> f32 {
        kick_at(120.0, t)
    }

    #[test]
    fn silence_stays_dark() {
        let mut beats = 0.0;
        let a = run(3.0, |_| 0.0, |f| beats = f.beat_count);
        let f = a.features();
        assert_eq!(beats, 0.0);
        assert!(f.bass < 1e-3 && f.mid < 1e-3 && f.treble < 1e-3 && f.volume < 1e-3);
        assert!(f.spectrum.iter().all(|&s| s < 1e-3));
        assert!(f.waveform.iter().all(|&s| s.abs() < 1e-3));
    }

    #[test]
    fn near_silent_noise_is_not_amplified() {
        // -70 dBFS hiss: well below the floor, must not light anything up.
        let mut seed = 1u32;
        let a = run(3.0, |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * 3e-4
        }, |_| {});
        let f = a.features();
        assert!(f.bass < 0.05 && f.treble < 0.05, "bass {} treble {}", f.bass, f.treble);
        assert_eq!(f.beat_count, 0.0);
    }

    #[test]
    fn detects_120_bpm_kick() {
        let a = run(12.0, kick_track, |_| {});
        let f = a.features();
        // 24 kicks in 12 s; allow a miss at the start while history fills.
        assert!((21.0..=25.0).contains(&f.beat_count), "beat_count {}", f.beat_count);
        assert!((f.bpm - 120.0).abs() < 2.0, "bpm {}", f.bpm);
        assert!(f.bpm_confidence > 0.3, "confidence {}", f.bpm_confidence);
    }

    #[test]
    fn tracks_tempos_other_than_the_default() {
        // 120 is the initial estimate, so prove the estimator actually moves.
        for target in [100.0f32, 140.0] {
            let a = run(14.0, |t| kick_at(target, t), |_| {});
            let f = a.features();
            assert!((f.bpm - target).abs() < 2.5, "target {target}, got {}", f.bpm);
        }
    }

    #[test]
    fn beat_envelope_decays_between_kicks() {
        let mut max_beat_late = 0.0f32;
        let mut hop = 0usize;
        run(4.0, kick_track, |f| {
            hop += 1;
            let t = hop as f32 * HOP as f32 / SR as f32;
            // 350–450 ms after each kick, the envelope must have mostly decayed.
            if t > 1.0 && (0.35..0.45).contains(&(t % 0.5)) {
                max_beat_late = max_beat_late.max(f.beat);
            }
        });
        assert!(max_beat_late < 0.1, "beat lingered at {max_beat_late}");
    }

    #[test]
    fn bass_responds_to_kick_and_releases() {
        let mut peak_bass = 0.0f32;
        let a = run(3.0, kick_track, |f| peak_bass = peak_bass.max(f.bass));
        assert!(peak_bass > 0.6, "peak bass {peak_bass}");
        // Then silence: everything should fall back toward zero quickly.
        let mut a = a;
        let zeros = vec![0.0; SR as usize];
        a.push(&zeros);
        assert!(a.features().bass < 0.02, "bass after 1s silence {}", a.features().bass);
        assert!(a.features().beat < 0.01);
    }

    #[test]
    fn steady_sine_has_stable_triggered_waveform() {
        // 441 Hz → exactly 100 samples per period; trigger alignment should
        // make consecutive waveforms near-identical.
        let mut prev: Option<[f32; WAVEFORM_SAMPLES]> = None;
        let mut max_diff = 0.0f32;
        let mut hop = 0;
        run(1.0, |t| 0.5 * (std::f32::consts::TAU * 441.0 * t).sin(), |f| {
            hop += 1;
            if hop > 40 {
                if let Some(p) = prev {
                    let d = p
                        .iter()
                        .zip(f.waveform.iter())
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0, f32::max);
                    max_diff = max_diff.max(d);
                }
                prev = Some(f.waveform);
            }
        });
        assert!(max_diff < 0.05, "waveform jitter {max_diff}");
    }

    #[test]
    fn waveform_ends_are_tapered() {
        let a = run(0.5, |t| 0.5 * (std::f32::consts::TAU * 200.0 * t).cos(), |_| {});
        let w = &a.features().waveform;
        assert!(w[0].abs() < 1e-3 && w[WAVEFORM_SAMPLES - 1].abs() < 1e-3);
    }

    #[test]
    fn treble_is_not_dwarfed_by_bass() {
        // Loud bass + quiet hi tone: per-band AGC should still give treble range.
        let mut peak_treble = 0.0f32;
        run(2.0, |t| {
            0.8 * (std::f32::consts::TAU * 60.0 * t).sin()
                + 0.02 * (std::f32::consts::TAU * 8000.0 * t).sin() * (t * 4.0).fract()
        }, |f| peak_treble = peak_treble.max(f.treble));
        assert!(peak_treble > 0.5, "peak treble {peak_treble}");
    }
}
