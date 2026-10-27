//! Audio feature extraction.
//!
//! [`Analyzer`] is a pure, deterministic DSP core: it consumes mono samples
//! and advances on a fixed hop of the *sample* clock, so every time constant
//! below is in seconds of audio rather than "per tick". The analysis thread
//! wraps it with I/O concerns — draining the capture ring, feeding synthetic
//! silence when WASAPI loopback stops delivering packets, and swapping in a
//! new source when capture reconnects.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use ringbuf::traits::Consumer as _;
use tracing::{debug, info, warn};
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

// Per-instrument onsets (kick / snare / hat): refractory s, envelope tau s,
// the fraction of the band's recent peak an onset must reach, and an
// absolute floor. Kick and snare use linear-magnitude flux (bass beating
// against a kick's tail, or a hat leaking into the snare band, is a large
// *log* change but a small linear one); hats use log flux so quiet ticks
// still register.
const KICK_TIMING: OnsetTiming = OnsetTiming { refractory: 0.12, tau: 0.10, rel: 0.4, floor: 0.002 };
const SNARE_TIMING: OnsetTiming = OnsetTiming { refractory: 0.10, tau: 0.12, rel: 0.4, floor: 0.0005 };
const HAT_TIMING: OnsetTiming = OnsetTiming { refractory: 0.06, tau: 0.05, rel: 0.25, floor: ONSET_MIN };
/// A hat onset this close to a snare onset is the snare's own noise.
const HAT_SNARE_MASK: f32 = 0.04;
/// `*_age` before the first onset, and its ceiling.
const AGE_MAX: f32 = 1000.0;

// Tempo estimation.
const TEMPO_WINDOW_S: f32 = 10.0;
const BPM_MIN: f32 = 80.0;
const BPM_MAX: f32 = 160.0;
const BPM_DEFAULT: f32 = 120.0;

// Room tuning (`Analyzer::set_room`), for a microphone across the room.
/// Per-bin power is smoothed this much before a noise floor is tracked
/// beneath it.
const ROOM_POWER_TAU: f32 = 0.10;
/// For this long after tuning starts, the floor simply follows the power:
/// the analysis window has only just filled, and a frame that boots in a
/// quiet room learns the room at once.
const ROOM_PRIME_S: f32 = 0.5;
/// No real microphone is ever perfectly silent; this long at digital zero
/// means it's muted, and the HUD says so.
const ROOM_NO_SIGNAL_S: f32 = 1.0;
/// The floor falls toward quieter moments with this time constant…
const ROOM_NOISE_FALL: f32 = 0.5;
/// …and creeps up under steady sound this slowly. A bassline or pad that
/// never dips is slowly taken for the room (20 dB above it in ~100 s); a
/// fan switching on is learned at the same pace.
const ROOM_NOISE_RISE_DB_PER_S: f32 = 0.2;
/// Subtract this multiple of the floor, so the floor's own flicker doesn't
/// show through as sparkle.
const ROOM_OVERSUB: f32 = 1.5;
/// Lift the cleaned signal's loud parts toward this level (dBFS, sine-peak
/// scale like `volume`)…
const ROOM_TARGET_DB: f32 = -12.0;
/// …but never so far that the room's noise floor lands within
/// `ROOM_NOISE_MARGIN_DB` of `FLOOR_DB` — and cut a hot mic until it
/// doesn't. Every absolute threshold below then means "above the room",
/// whatever the mic's gain. (The band AGC stretches anything that crosses
/// the floor to full range, so the noise must sit under it, not at it.)
const ROOM_NOISE_MARGIN_DB: f32 = 3.0;
const ROOM_MAX_GAIN_DB: f32 = 36.0;
const ROOM_MAX_CUT_DB: f32 = 40.0;
/// The level being lifted rises quickly and falls slowly (tens of seconds),
/// so a quiet passage isn't pumped up to match a loud one.
const ROOM_LEVEL_RISE: f32 = 1.0;
const ROOM_LEVEL_FALL: f32 = 20.0;
/// Gain eases toward its target over seconds, faster when it must come down.
const ROOM_GAIN_RISE: f32 = 3.0;
const ROOM_GAIN_FALL: f32 = 0.5;

// Waveform.
const WAVE_SEARCH: usize = 512;
const WAVE_TAPER: usize = 24;
const WAVE_REF_RELEASE: f32 = 2.0;

/// A capture stream's sample ring + its rate. Sent to the analysis thread
/// whenever capture (re)connects.
pub struct AudioSource {
    pub consumer: ringbuf::HeapCons<f32>,
    pub sample_rate: u32,
    /// Room tuning for a microphone across the room ([`Analyzer::set_room`]).
    pub room: bool,
}

/// What room tuning is doing, for the HUD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RoomStatus {
    /// Just started: learning the room's noise floor.
    Learning,
    /// The input has been digital silence for a while — a muted mic, a
    /// hardware mute key, a privacy shutter.
    NoSignal,
    /// Gain applied (dB) and the room's noise floor (dBFS, before gain).
    Tuned { gain_db: f32, noise_db: f32 },
}

/// Room tuning's status, published by the analysis thread for the HUD. Not
/// part of `AudioFeatures`: presets have no use for it.
#[derive(Default)]
pub struct RoomMeter(Mutex<Option<RoomStatus>>);

impl RoomMeter {
    fn set(&self, status: Option<RoomStatus>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = status;
    }

    /// `None` while room tuning is off.
    pub fn get(&self) -> Option<RoomStatus> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Spawn the analysis thread. Returns the features reader for the render
/// thread, a sender for (re)attaching audio sources, and room tuning's meter.
pub fn spawn_analysis() -> (Output<AudioFeatures>, Sender<AudioSource>, Arc<RoomMeter>) {
    let (input, output) = TripleBuffer::new(&AudioFeatures::default()).split();
    let (tx, rx) = channel();
    let meter = Arc::new(RoomMeter::default());

    let m = meter.clone();
    thread::Builder::new()
        .name("seoul-analysis".into())
        .spawn(move || analysis_loop(rx, input, &m))
        .expect("spawn analysis thread");

    (output, tx, meter)
}

fn analysis_loop(sources: Receiver<AudioSource>, mut out: Input<AudioFeatures>, meter: &RoomMeter) {
    // Until a source arrives we run at a nominal rate on synthesized silence.
    let mut source: Option<ringbuf::HeapCons<f32>> = None;
    let mut analyzer = Analyzer::new(48_000);
    let mut buf: Vec<f32> = Vec::with_capacity(8192);
    let zeros = vec![0.0f32; 4096];

    let mut last_real = Instant::now();
    let mut silence_clock = Instant::now();
    // For RUST_LOG=seoul=debug: the input's peak level every few seconds.
    let mut level_peak = 0.0f32;
    let mut level_clock = Instant::now();
    let mut was_silent = false;

    loop {
        // Latest source wins; a sample-rate change rebuilds the analyzer.
        while let Ok(src) = sources.try_recv() {
            if src.sample_rate as f32 != analyzer.sample_rate {
                analyzer = Analyzer::new(src.sample_rate);
            }
            analyzer.set_room(src.room);
            source = Some(src.consumer);
            info!(sample_rate = src.sample_rate, room = src.room, "analysis attached to audio source");
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
            level_peak = buf.iter().fold(level_peak, |m, s| m.max(s.abs()));
            if now.duration_since(level_clock) > Duration::from_secs(5) {
                let db = 20.0 * (level_peak + 1e-12).log10();
                debug!(peak_dbfs = format!("{db:.1}"), room = ?analyzer.room_status(), "input level");
                (level_peak, level_clock) = (0.0, now);
            }
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
            let status = analyzer.room_status();
            let silent = status == Some(RoomStatus::NoSignal);
            if silent != was_silent {
                if silent {
                    warn!("the input is digital silence — is the mic muted (a mute key, a privacy shutter)?");
                } else {
                    info!("the input has signal again");
                }
                was_silent = silent;
            }
            meter.set(status);
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
    prev_mags: Vec<f32>,

    rolling: Vec<f32>,
    write_pos: usize,
    pending: usize,
    frame: Vec<f32>,

    spectrum_map: Vec<(usize, usize)>,
    spectrum_tilt_db: Vec<f32>,
    bands: [(usize, usize); 3],
    kick_range: (usize, usize),
    snare_range: (usize, usize),
    hat_range: (usize, usize),
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
    /// kick, snare, hat.
    drums: [OnsetDetector; 3],

    clock: f32,
    onset_times: VecDeque<f32>,
    bpm: f32,
    bpm_confidence: f32,
    phase: f32,

    room: Option<Room>,

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
            prev_mags: vec![0.0; n_bins],
            rolling: vec![0.0; FFT_SIZE],
            write_pos: 0,
            pending: 0,
            frame: vec![0.0; FFT_SIZE],
            spectrum_map,
            spectrum_tilt_db,
            bands,
            kick_range: (hz(30.0), hz(180.0)),
            snare_range: (hz(1000.0), hz(4000.0)),
            hat_range: (hz(7000.0), hz(16000.0)),
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
            drums: [
                OnsetDetector::new(onset_hist_len, KICK_TIMING),
                OnsetDetector::new(onset_hist_len, SNARE_TIMING),
                OnsetDetector::new(onset_hist_len, HAT_TIMING),
            ],
            clock: 0.0,
            onset_times: VecDeque::new(),
            bpm: BPM_DEFAULT,
            bpm_confidence: 0.0,
            phase: 0.0,
            room: None,
            features: AudioFeatures::default(),
        }
    }

    pub fn features(&self) -> &AudioFeatures {
        &self.features
    }

    /// Turn room tuning on or off. Off (loopback, the synth) is a direct
    /// feed; on, the analyzer learns the room's noise floor, subtracts it,
    /// and slowly lifts what's left (see [`Room`]). Turning it on again
    /// starts learning afresh.
    pub fn set_room(&mut self, on: bool) {
        if on != self.room.is_some() {
            self.room = on.then(|| Room::new(self.mags.len(), self.bands));
        }
    }

    /// Room tuning's current gain and the room's noise floor (dBFS, before
    /// gain), when it's on and has heard something.
    #[cfg(test)]
    fn room_stats(&self) -> Option<(f32, f32)> {
        self.room.as_ref().filter(|r| r.primed).map(|r| (r.gain_db, r.noise_db))
    }

    /// `None` while room tuning is off.
    pub fn room_status(&self) -> Option<RoomStatus> {
        let r = self.room.as_ref()?;
        Some(if r.silent > ROOM_NO_SIGNAL_S {
            RoomStatus::NoSignal
        } else if !r.primed {
            RoomStatus::Learning
        } else {
            RoomStatus::Tuned { gain_db: r.gain_db, noise_db: r.noise_db }
        })
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
        if let Some(room) = &mut self.room {
            room.process(&mut self.mags, &self.frame, dt);
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
        // Detected beats aren't bar-aligned; a live Soba link replaces this.
        f.bar = (self.beat_count / 4) as f32;
        f.bpm = self.bpm;
        f.bpm_confidence = self.bpm_confidence;
        f.beat_phase = self.phase;
        let [k, s, h] = &self.drums;
        (f.kick, f.snare, f.hat) = (k.env, s.env, h.env);
        (f.kick_age, f.snare_age, f.hat_age) = (k.age, s.age, h.age);
        (f.kick_count, f.snare_count, f.hat_count) = (k.hits as f32, s.hits as f32, h.hits as f32);
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
        // In a room, only the part that isn't the room's noise, lifted.
        let room = self.room.as_ref().map_or(1.0, |r| r.presence * r.gain);
        let rms = rms * std::f32::consts::SQRT_2 * room;
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
        // Snares and claps are noise centered in the upper mids; hats live
        // above 7 kHz. Each band gets its own detector.
        let hat = flux(self.hat_range, &self.log_mags, &self.prev_log_mags);
        let kick_lin = flux(self.kick_range, &self.mags, &self.prev_mags);
        let snare = flux(self.snare_range, &self.mags, &self.prev_mags);
        self.prev_mags.copy_from_slice(&self.mags);
        // In a room, what's left after the noise floor still flickers, and
        // log flux turns flicker into onsets. Count onsets only as far as
        // the frame is signal rather than room.
        let gate = self.room.as_ref().map_or(1.0, |r| smoothstep(0.3, 0.7, r.presence));
        let (kick, broad, hat, kick_lin, snare) =
            (kick * gate, broad * gate, hat * gate, kick_lin * gate, snare * gate);
        self.drums[0].step(kick_lin, dt);
        let snare_hit = self.drums[1].step(snare, dt);
        let hat_hit = self.drums[2].step(hat, dt);
        // A clap's noise reaches the hat band too, and its log flux can peak
        // a hop before the snare band's: whichever fires first, the hat yields.
        if (snare_hit || hat_hit)
            && self.drums[1].since < HAT_SNARE_MASK
            && self.drums[2].since < HAT_SNARE_MASK
        {
            self.drums[2].retract();
        }
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
        // The waveform normalizes its own height, which would draw a quiet
        // room's hiss full size. Scale it by how much of the frame is signal.
        let gain = gain * self.room.as_ref().map_or(1.0, |r| r.presence);

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

/// Adaptive-threshold peak picker for one band's onset strength, with a
/// decaying 0..1 envelope. Same scheme as the main beat detector, minus
/// the tempo tracking.
#[derive(Clone, Copy)]
struct OnsetTiming {
    refractory: f32,
    tau: f32,
    rel: f32,
    floor: f32,
}

struct OnsetDetector {
    hist: VecDeque<f32>,
    hist_len: usize,
    prev: [f32; 2],
    since: f32,
    timing: OnsetTiming,
    env: f32,
    /// Seconds since the last (unretracted) onset, and its value before
    /// the latest one so a retraction can restore it.
    age: f32,
    age_before: f32,
    /// Onsets reported so far, net of retractions.
    hits: u32,
    retracted: bool,
}

impl OnsetDetector {
    fn new(hist_len: usize, timing: OnsetTiming) -> Self {
        Self {
            hist: VecDeque::with_capacity(hist_len + 1),
            hist_len,
            prev: [0.0; 2],
            since: f32::INFINITY,
            timing,
            env: 0.0,
            age: AGE_MAX,
            age_before: AGE_MAX,
            hits: 0,
            retracted: false,
        }
    }

    /// Withdraw the most recent onset (it belonged to another instrument).
    /// Its refractory period still holds, so the same sound can't re-fire.
    fn retract(&mut self) {
        if !self.retracted {
            self.retracted = true;
            self.env = 0.0;
            self.age = (self.age_before + self.age).min(AGE_MAX);
            self.hits -= 1;
        }
    }

    /// Feed this hop's onset strength. Returns true on a detected onset.
    fn step(&mut self, onset: f32, dt: f32) -> bool {
        let n = self.hist.len().max(1) as f32;
        let mean = self.hist.iter().sum::<f32>() / n;
        let var = self.hist.iter().map(|o| (o - mean).powi(2)).sum::<f32>() / n;
        let peak = self.hist.iter().fold(0.0f32, |m, &o| m.max(o));
        let threshold = (mean + ONSET_K * var.sqrt() + self.timing.floor).max(peak * self.timing.rel);

        let [prev2, prev1] = self.prev;
        self.since += dt;
        self.age = (self.age + dt).min(AGE_MAX);
        self.env *= (-dt / self.timing.tau).exp();
        let hit = prev1 > prev2 && prev1 >= onset && prev1 > threshold && self.since > self.timing.refractory;
        if hit {
            self.since = 0.0;
            self.env = 1.0;
            self.age_before = self.age;
            self.age = 0.0;
            self.hits += 1;
            self.retracted = false;
        }

        self.prev = [prev1, onset];
        self.hist.push_back(onset);
        if self.hist.len() > self.hist_len {
            self.hist.pop_front();
        }
        hit
    }
}

/// Room tuning for a microphone across the room, which hears the music
/// quieter, thinner and over a bed of noise (fans, traffic, its own hiss).
///
/// - A noise floor per FFT bin is tracked under the smoothed power — it
///   falls quickly into quiet moments and creeps up slowly under sound —
///   and subtracted from every bin before bands, spectrum and onsets see it.
/// - A slow gain lifts the cleaned signal's loud parts toward
///   `ROOM_TARGET_DB`, but never lifts the noise floor above `FLOOR_DB`,
///   so the analysis's absolute thresholds all sit just above the room.
///   It holds while the room is quiet, ready for the next record.
/// - `presence` (how much of the frame is signal, 0..1) scales volume and
///   the waveform, which are computed from raw samples.
struct Room {
    bands: [(usize, usize); 3],
    /// Smoothed power per bin and the noise floor beneath it (pre-gain).
    power: Vec<f32>,
    noise: Vec<f32>,
    /// The same for the frame's mean square.
    frame_power: f32,
    frame_noise: f32,
    /// Level of the cleaned signal's loud parts, dB; unknown until heard.
    level_db: Option<f32>,
    gain_db: f32,
    gain: f32,
    noise_db: f32,
    presence: f32,
    /// Seconds of sound heard since tuning started, and of digital silence
    /// since sound was last heard.
    heard: f32,
    silent: f32,
    primed: bool,
}

impl Room {
    fn new(n_bins: usize, bands: [(usize, usize); 3]) -> Self {
        Self {
            bands,
            power: vec![0.0; n_bins],
            noise: vec![0.0; n_bins],
            frame_power: 0.0,
            frame_noise: 0.0,
            level_db: None,
            gain_db: 0.0,
            gain: 1.0,
            noise_db: FLOOR_DB,
            presence: 0.0,
            heard: 0.0,
            silent: 0.0,
            primed: false,
        }
    }

    /// Learn from this hop, then clean `mags` in place: noise subtracted,
    /// gain applied.
    fn process(&mut self, mags: &mut [f32], frame: &[f32], dt: f32) {
        let ms = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        // Digital silence (a muted mic, the analysis thread's stand-in
        // zeros) says nothing about the room: hold what was learned.
        if ms <= 1e-14 {
            self.silent += dt;
        } else {
            self.silent = 0.0;
            self.heard += dt;
            let priming = self.heard < ROOM_PRIME_S;
            let smooth = 1.0 - (-dt / ROOM_POWER_TAU).exp();
            let fall = 1.0 - (-dt / ROOM_NOISE_FALL).exp();
            let rise = 10f32.powf(ROOM_NOISE_RISE_DB_PER_S * dt / 10.0);
            let track = |power: &mut f32, noise: &mut f32, p: f32| {
                *power += (p - *power) * smooth;
                if priming {
                    *noise = *power;
                } else if *power < *noise {
                    *noise += (*power - *noise) * fall;
                } else {
                    *noise = (*noise * rise).min(*power);
                }
            };
            for ((m, power), noise) in mags.iter().zip(&mut self.power).zip(&mut self.noise) {
                track(power, noise, m * m);
            }
            track(&mut self.frame_power, &mut self.frame_noise, ms);
            self.primed = !priming;
        }

        // The loudest the noise gets in any one band (or the whole frame)
        // decides how far the gain may lift without waking the floor.
        let band_noise = self
            .bands
            .iter()
            .map(|&(lo, hi)| self.noise[lo..hi].iter().sum::<f32>())
            .fold(2.0 * self.frame_noise, f32::max);
        self.noise_db = 10.0 * (band_noise + 1e-12).log10();

        let clean_ms = (ms - ROOM_OVERSUB * self.frame_noise).max(0.0);
        self.presence = if ms > 0.0 { (clean_ms / ms).sqrt() } else { 0.0 };
        // Only signal well clear of the room moves the level: silence and
        // hiss leave the gain where the last music put it.
        if self.presence > 0.5 {
            let db = 10.0 * (2.0 * clean_ms + 1e-12).log10();
            self.level_db = Some(match self.level_db {
                Some(l) => follow(l, db, dt, ROOM_LEVEL_RISE, ROOM_LEVEL_FALL),
                None => db,
            });
        }
        let ceiling = FLOOR_DB - ROOM_NOISE_MARGIN_DB - self.noise_db;
        let target = self
            .level_db
            .map_or(0.0, |l| (ROOM_TARGET_DB - l).max(0.0))
            .min(ceiling)
            .clamp(-ROOM_MAX_CUT_DB, ROOM_MAX_GAIN_DB);
        self.gain_db = follow(self.gain_db, target, dt, ROOM_GAIN_RISE, ROOM_GAIN_FALL);
        self.gain = db_to_amp(self.gain_db);

        for (m, noise) in mags.iter_mut().zip(&self.noise) {
            *m = (*m * *m - ROOM_OVERSUB * noise).max(0.0).sqrt() * self.gain;
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
fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
        assert!(a.drums.iter().all(|d| d.hits == 0), "drum hits on hiss");
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
    fn drums_are_told_apart() {
        // The --synth track at 124 BPM: kick on every beat, clap on 2 and 4,
        // hats on the off-beat eighths; bar 8 drops the kick. Count onsets
        // per instrument over bars 2–7 (6 bars = 24 beats).
        use crate::audio::synth::{SAMPLE_RATE, Synth};
        let mut synth = Synth::default();
        let mut a = Analyzer::new(SAMPLE_RATE);
        let beat_len = 60.0 / 124.0;
        let samples = |beats: f32| (beats * beat_len * SAMPLE_RATE as f32) as usize;
        // Start just before a beat so its onset lands inside the window.
        let warmup: Vec<f32> = (0..samples(3.9)).map(|_| synth.next_sample()).collect();
        a.push(&warmup);
        let start = a.drums.each_ref().map(|d| d.hits);
        let body: Vec<f32> = (0..samples(24.0)).map(|_| synth.next_sample()).collect();
        a.push(&body);
        let [kick, snare, hat] = [0, 1, 2].map(|i| a.drums[i].hits - start[i]);
        assert!((23..=25).contains(&kick), "kick {kick} (expect 24)");
        assert!((11..=13).contains(&snare), "snare {snare} (expect 12)");
        assert!((23..=25).contains(&hat), "hat {hat} (expect 24)");

        // Ages and counts reach the features. The run ends 0.9 beats
        // (0.44 s) after the last kick and 0.4 beats (0.19 s) after the last
        // off-beat hat, less a few ms of detection latency.
        let f = a.features();
        assert_eq!(f.kick_count, a.drums[0].hits as f32);
        assert!((0.35..0.45).contains(&f.kick_age), "kick_age {}", f.kick_age);
        assert!((0.12..0.2).contains(&f.hat_age), "hat_age {}", f.hat_age);
    }

    /// What the analyzer made of `seconds` of a signal, over its last
    /// `window` seconds.
    #[derive(Debug)]
    #[allow(dead_code)] // some fields only reach room_report's Debug output
    struct Heard {
        bpm: f32,
        conf: f32,
        beats: f32,
        kicks: u32,
        /// Means of bass, mid, treble, volume over the window.
        mean: [f32; 4],
        /// 95th percentiles of the same.
        p95: [f32; 4],
        spectrum_mean: f32,
        wave_peak: f32,
        room: Option<(f32, f32)>,
    }

    fn hear(seconds: f32, window: f32, room: bool, mut next: impl FnMut() -> f32) -> Heard {
        use crate::audio::synth::SAMPLE_RATE;
        let mut a = Analyzer::new(SAMPLE_RATE);
        a.set_room(room);
        let lead = ((seconds - window) * SAMPLE_RATE as f32) as usize;
        let lead: Vec<f32> = (0..lead).map(|_| next()).collect();
        a.push(&lead);
        let (beats0, kicks0) = (a.features().beat_count, a.drums[0].hits);
        let mut series: [Vec<f32>; 4] = Default::default();
        let (mut spec, mut wave_peak) = (0.0, 0.0f32);
        let hops = (window * SAMPLE_RATE as f32) as usize / HOP;
        for _ in 0..hops {
            let chunk: Vec<f32> = (0..HOP).map(|_| next()).collect();
            a.push(&chunk);
            let f = a.features();
            for (s, v) in series.iter_mut().zip([f.bass, f.mid, f.treble, f.volume]) {
                s.push(v);
            }
            spec += f.spectrum.iter().sum::<f32>() / SPECTRUM_BINS as f32;
            wave_peak = wave_peak.max(f.waveform.iter().fold(0.0, |m, s| m.max(s.abs())));
        }
        let mean = series.each_ref().map(|s| s.iter().sum::<f32>() / s.len() as f32);
        let p95 = series.map(|mut s| {
            s.sort_by(f32::total_cmp);
            s[s.len() * 95 / 100]
        });
        let f = a.features();
        Heard {
            bpm: f.bpm,
            conf: f.bpm_confidence,
            beats: f.beat_count - beats0,
            kicks: a.drums[0].hits - kicks0,
            mean,
            p95,
            spectrum_mean: spec / hops as f32,
            wave_peak,
            room: a.room_stats(),
        }
    }

    fn track(room_sim: bool) -> impl FnMut() -> f32 {
        let mut t = crate::audio::synth::TestTrack::new(room_sim);
        move || t.next_sample()
    }

    /// The test track through the simulated room, at a mic gain.
    fn room_track(mic_db: f32) -> impl FnMut() -> f32 {
        let mut s = crate::audio::synth::Synth::default();
        let mut r = crate::audio::synth::RoomSim::new(mic_db);
        move || r.hear(s.next_sample())
    }

    /// The simulated room with nothing playing.
    fn room_noise(mic_db: f32) -> impl FnMut() -> f32 {
        let mut r = crate::audio::synth::RoomSim::new(mic_db);
        move || r.hear(0.0)
    }

    /// `cargo test --release room_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn room_report() {
        let rms = |mut f: Box<dyn FnMut() -> f32>| {
            let n = 48_000 * 8;
            let ms = (0..n).map(|_| f().powi(2)).sum::<f32>() / n as f32;
            10.0 * ms.log10()
        };
        for mic in [-20.0, 0.0, 20.0] {
            println!(
                "mic {mic:+} dB: track {:.1} dBFS rms, noise {:.1}",
                rms(Box::new(room_track(mic))),
                rms(Box::new(room_noise(mic)))
            );
            println!("  room, untuned  {:?}", hear(20.0, 8.0, false, room_track(mic)));
            println!("  room, tuned    {:?}", hear(20.0, 8.0, true, room_track(mic)));
            println!("  noise, untuned {:?}", hear(20.0, 8.0, false, room_noise(mic)));
            println!("  noise, tuned   {:?}", hear(20.0, 8.0, true, room_noise(mic)));
        }
        println!("direct {:.1} dBFS rms {:?}", rms(Box::new(track(false))), hear(20.0, 8.0, false, track(false)));
        // How the learned floor compares to the noise's true mean power.
        let mut a = Analyzer::new(crate::audio::synth::SAMPLE_RATE);
        a.set_room(true);
        let mut n = room_noise(0.0);
        let mut mean = vec![0.0f32; a.mags.len()];
        let mut ratio = vec![0.0f32; a.mags.len()];
        for i in 0..2000 {
            let chunk: Vec<f32> = (0..HOP).map(|_| n()).collect();
            a.push(&chunk);
            if i >= 1000 {
                let r = a.room.as_ref().unwrap();
                for b in 1..mean.len() {
                    mean[b] += r.power[b];
                    ratio[b] += r.noise[b];
                }
            }
        }
        let q: f32 = (5..900).map(|b| ratio[b] / mean[b]).sum::<f32>() / 895.0;
        println!("learned floor / mean noise power = {q:.2}");
    }

    #[test]
    fn room_tuning_wakes_a_quiet_mic() {
        // A laptop mic across the room: the record lands under the analysis
        // floor. Tempo survives (log flux is level-blind) but every band
        // reads zero, so the picture sits dark. Tuned, it moves and locks.
        let mic = crate::audio::synth::ROOM_SIM_MIC_DB;
        let dark = hear(20.0, 8.0, false, room_track(mic));
        assert!(dark.p95.iter().all(|&v| v < 0.01), "untuned should be dark: {dark:?}");
        let h = hear(20.0, 8.0, true, room_track(mic));
        assert!((h.bpm - 124.0).abs() < 2.0 && h.conf > 0.5, "{h:?}");
        assert!(h.p95[0] > 0.5 && h.p95[1] > 0.4 && h.p95[2] > 0.4, "bands barely move: {h:?}");
        assert!(h.wave_peak > 0.2, "{h:?}");
        let (gain_db, _) = h.room.unwrap();
        assert!(gain_db > 10.0, "gain {gain_db}");
    }

    #[test]
    fn room_tuning_quiets_a_hot_mic() {
        // A hot mic in an empty room: untuned, its noise is above the floor
        // and the AGC stretches it into a twitching picture. Tuned, the gain
        // comes down until the room sits under the floor.
        let twitch = hear(12.0, 6.0, false, room_noise(20.0));
        assert!(twitch.mean.iter().all(|&v| v > 0.5), "expected untuned twitching: {twitch:?}");
        let h = hear(12.0, 6.0, true, room_noise(20.0));
        assert!(h.mean.iter().all(|&v| v < 0.02) && h.beats == 0.0, "{h:?}");
        let (gain_db, _) = h.room.unwrap();
        assert!(gain_db < -5.0, "gain {gain_db}");
    }

    #[test]
    fn empty_room_stays_dark_at_any_mic_gain() {
        for mic in [-20.0, 0.0, 20.0] {
            let h = hear(12.0, 6.0, true, room_noise(mic));
            assert!(h.mean.iter().all(|&v| v < 0.02), "mic {mic}: {h:?}");
            assert!(h.p95.iter().all(|&v| v < 0.05), "mic {mic}: {h:?}");
            assert!(h.beats <= 1.0 && h.wave_peak < 0.05 && h.spectrum_mean < 0.05, "mic {mic}: {h:?}");
        }
    }

    #[test]
    fn room_tuning_hears_the_same_record_at_any_mic_gain() {
        let quiet = hear(20.0, 8.0, true, room_track(-20.0));
        let hot = hear(20.0, 8.0, true, room_track(20.0));
        assert!((quiet.bpm - hot.bpm).abs() < 1.0, "{quiet:?} vs {hot:?}");
        for b in 0..4 {
            assert!((quiet.mean[b] - hot.mean[b]).abs() < 0.05, "band {b}: {quiet:?} vs {hot:?}");
        }
    }

    #[test]
    fn room_floor_survives_a_muted_mic() {
        // Digital silence must not teach the room "nothing": when the mic
        // comes back, the floor and gain are where they were.
        let mut a = Analyzer::new(crate::audio::synth::SAMPLE_RATE);
        a.set_room(true);
        let mut n = room_noise(0.0);
        let noise: Vec<f32> = (0..48_000 * 6).map(|_| n()).collect();
        a.push(&noise);
        let (_, before) = a.room_stats().unwrap();
        a.push(&vec![0.0; 48_000 * 6]);
        assert_eq!(a.room_status(), Some(RoomStatus::NoSignal));
        let (_, after) = a.room_stats().unwrap();
        assert!((before - after).abs() < 1.0, "floor {before} → {after} dB");
        let noise: Vec<f32> = (0..48_000 * 2).map(|_| n()).collect();
        a.push(&noise);
        assert!(matches!(a.room_status(), Some(RoomStatus::Tuned { .. })));
        let f = a.features();
        assert!(f.bass < 0.05 && f.volume < 0.05 && f.beat_count == 0.0, "woke on return: {f:?}");
    }

    #[test]
    fn direct_feeds_are_untouched_by_room_code() {
        // Loopback and the synth never turn tuning on; make sure turning it
        // on and off again leaves an analyzer that matches a fresh one.
        let mut a = Analyzer::new(crate::audio::synth::SAMPLE_RATE);
        a.set_room(true);
        a.set_room(false);
        let mut b = Analyzer::new(crate::audio::synth::SAMPLE_RATE);
        let mut t = track(false);
        let s: Vec<f32> = (0..48_000 * 4).map(|_| t()).collect();
        a.push(&s);
        b.push(&s);
        assert_eq!(a.features().bass, b.features().bass);
        assert_eq!(a.features().beat_count, b.features().beat_count);
        assert!(a.room_stats().is_none());
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
