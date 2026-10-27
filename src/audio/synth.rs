//! `--synth`: a deterministic built-in test track that stands in for loopback
//! capture. 124 BPM, A-minor: kick, clap, hats, a ducked bassline, a pad and a
//! sixteenth-note arpeggio, with a kick-less breakdown every eighth bar so
//! silence-ish decay and beat re-acquisition get exercised too.

use std::f32::consts::TAU;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use tracing::info;

use crate::audio::analysis::AudioSource;
use crate::audio::capture::RING_CAPACITY;

pub const SAMPLE_RATE: u32 = 48_000;
const BPM: f32 = 124.0;
/// Stay this far ahead of real time so the analysis thread never starves.
const LEAD_SAMPLES: usize = 2048;

/// Play the test track in real time. `room_sim` plays it as a microphone
/// across the room hears it; `room` asks the analyzer for room tuning.
pub fn spawn_synth(sink: Sender<AudioSource>, room_sim: bool, room: bool) {
    let (mut producer, consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
    let _ = sink.send(AudioSource {
        consumer,
        sample_rate: SAMPLE_RATE,
        room,
    });
    info!(bpm = BPM, room_sim, room, "synth test signal running");

    thread::Builder::new()
        .name("seoul-synth".into())
        .spawn(move || {
            let mut synth = TestTrack::new(room_sim);
            let start = Instant::now();
            let mut produced = 0usize;
            loop {
                let due = (start.elapsed().as_secs_f32() * SAMPLE_RATE as f32) as usize + LEAD_SAMPLES;
                while produced < due {
                    // A full ring means analysis is behind; dropping is fine.
                    let _ = producer.try_push(synth.next_sample());
                    produced += 1;
                }
                thread::sleep(Duration::from_millis(4));
            }
        })
        .expect("spawn synth thread");
}

/// A stand-in Soba playing along with the track over real OSC, so the link's
/// receiver, timeline and clock lock all run end to end (`audio/link_fake.rs`).
pub fn spawn_fake_link(port: u16) {
    info!(port, "stand-in Soba link running");
    thread::Builder::new()
        .name("seoul-fake-link".into())
        .spawn(move || {
            let version = format!("seoul synth {}", env!("CARGO_PKG_VERSION"));
            if let Err(e) = super::link_fake::run(port, Instant::now(), &version) {
                tracing::warn!("stand-in Soba link stopped: {e}");
            }
        })
        .expect("spawn fake link thread");
}

/// The test track as Seoul hears it: straight from the generator, or
/// through [`RoomSim`]'s microphone across the room.
pub struct TestTrack {
    synth: Synth,
    room: Option<RoomSim>,
}

impl TestTrack {
    pub fn new(room_sim: bool) -> Self {
        Self {
            synth: Synth::default(),
            room: room_sim.then(RoomSim::default),
        }
    }

    pub fn next_sample(&mut self) -> f32 {
        let s = self.synth.next_sample();
        match &mut self.room {
            Some(room) => room.hear(s),
            None => s,
        }
    }
}

/// Room level: a record across the room reaches the mic about 30 dB down.
const ROOM_LEVEL: f32 = 0.032;
/// Small speakers and a thin mic: two first-order high-passes (12 dB/oct).
const ROOM_HIGHPASS_HZ: f32 = 150.0;
/// Early reflections off nearby walls: (delay s, gain).
const ROOM_REFLECTIONS: [(f32, f32); 4] = [(0.011, 0.5), (0.023, 0.35), (0.041, 0.25), (0.067, 0.15)];
/// Steady noise in RMS: fan and traffic rumble below ~400 Hz, plus preamp hiss.
const ROOM_RUMBLE_DB: f32 = -64.0;
const ROOM_HISS_DB: f32 = -72.0;
const ROOM_RUMBLE_HZ: f32 = 400.0;

/// The default room's mic gain: a laptop or webcam mic, music at living-room
/// volume, which lands the record below the analysis floor if untuned.
pub const ROOM_SIM_MIC_DB: f32 = -20.0;

/// What a microphone across a living room hears of a signal: about 30 dB
/// quieter, almost no bass, a few early reflections and a steady bed of
/// rumble and hiss — a broadband signal-to-noise ratio of roughly 20 dB.
/// The mic's gain scales all of it: low, and the music sinks under the
/// analysis floor; high, and the room's noise rises above it.
/// Deterministic, like the track, so tests and recordings repeat.
pub struct RoomSim {
    mic: f32,
    hp_x: [f32; 2],
    hp_y: [f32; 2],
    hp_a: f32,
    echo: Vec<f32>,
    pos: usize,
    taps: [(usize, f32); 4],
    noise: u32,
    rumble: f32,
    rumble_k: f32,
    rumble_gain: f32,
    hiss_gain: f32,
}

impl Default for RoomSim {
    fn default() -> Self {
        Self::new(ROOM_SIM_MIC_DB)
    }
}

impl RoomSim {
    pub fn new(mic_db: f32) -> Self {
        let sr = SAMPLE_RATE as f32;
        let rc = 1.0 / (TAU * ROOM_HIGHPASS_HZ);
        let rumble_k = 1.0 - (-TAU * ROOM_RUMBLE_HZ / sr).exp();
        // Uniform white noise in -1..1 has RMS 1/√3; a one-pole low-pass
        // with coefficient k keeps √(k / (2 − k)) of it.
        let white_rms = 1.0 / 3f32.sqrt();
        let rumble_rms = white_rms * (rumble_k / (2.0 - rumble_k)).sqrt();
        let db = |d: f32| 10f32.powf(d / 20.0);
        Self {
            mic: db(mic_db),
            hp_x: [0.0; 2],
            hp_y: [0.0; 2],
            hp_a: rc / (rc + 1.0 / sr),
            echo: vec![0.0; 4096],
            pos: 0,
            taps: ROOM_REFLECTIONS.map(|(d, g)| ((d * sr) as usize, g)),
            noise: 0x9e37_79b9,
            rumble: 0.0,
            rumble_k,
            rumble_gain: db(ROOM_RUMBLE_DB) / rumble_rms,
            hiss_gain: db(ROOM_HISS_DB) / white_rms,
        }
    }

    fn white(&mut self) -> f32 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    pub fn hear(&mut self, s: f32) -> f32 {
        let mut x = s;
        for i in 0..2 {
            let y = self.hp_a * (self.hp_y[i] + x - self.hp_x[i]);
            self.hp_x[i] = x;
            self.hp_y[i] = y;
            x = y;
        }
        let len = self.echo.len();
        self.echo[self.pos] = x;
        let mut wet = x;
        for &(d, g) in &self.taps {
            wet += g * self.echo[(self.pos + len - d) % len];
        }
        self.pos = (self.pos + 1) % len;

        let w = self.white();
        self.rumble += (w - self.rumble) * self.rumble_k;
        let hiss = self.white();
        self.mic * (ROOM_LEVEL * wet + self.rumble_gain * self.rumble + self.hiss_gain * hiss)
    }
}

/// The test track's generator. Public so `--record` can step it in lockstep
/// with rendering instead of in real time.
#[derive(Default)]
pub struct Synth {
    n: u64,
    noise: u32,
    hat_prev: f32,
}

const BASS_NOTES: [f32; 4] = [55.0, 43.65, 65.41, 49.0]; // A1 F1 C2 G1
const CHORDS: [[f32; 3]; 4] = [
    [220.0, 261.63, 329.63], // Am
    [174.61, 220.0, 261.63], // F
    [261.63, 329.63, 392.0], // C
    [196.0, 246.94, 293.66], // G
];

impl Synth {
    fn white(&mut self) -> f32 {
        // xorshift32
        let mut x = self.noise.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    pub fn next_sample(&mut self) -> f32 {
        let t = self.n as f32 / SAMPLE_RATE as f32;
        self.n += 1;

        let beat_len = 60.0 / BPM;
        let beat_f = t / beat_len;
        let beat = beat_f.floor() as u64;
        let bt = (beat_f - beat as f32) * beat_len; // seconds since beat
        let bar = (beat / 4) as usize;
        let beat_in_bar = beat % 4;
        let breakdown = bar % 8 == 7;
        let chord = bar % 4;

        let mut out = 0.0;

        // Kick (off during breakdown bars).
        let kick_env = (-bt * 16.0).exp();
        if !breakdown {
            let f = 48.0 + 110.0 * (-bt * 35.0).exp();
            out += 0.9 * kick_env * (TAU * f * bt).sin();
        }

        // Clap on 2 and 4.
        let noise = self.white();
        if beat_in_bar % 2 == 1 {
            out += 0.35 * (-bt * 22.0).exp() * noise;
        }

        // Off-beat hats: differentiated noise ≈ high-passed.
        let hp = noise - self.hat_prev;
        self.hat_prev = noise;
        let eighth = beat_len * 0.5;
        let et = (t % eighth) / eighth;
        if et > 0.0 && ((t / eighth) as u64) % 2 == 1 {
            out += 0.12 * (-et * eighth * 70.0).exp() * hp;
        }

        // Bass: a few saw harmonics, ducked by the kick.
        let bf = BASS_NOTES[chord];
        let saw = (1..=4).map(|h| (TAU * bf * h as f32 * t).sin() / h as f32).sum::<f32>();
        let duck = if breakdown { 1.0 } else { 1.0 - 0.8 * kick_env };
        out += 0.22 * saw * duck;

        // Pad: slow-breathing chord.
        let swell = 0.6 + 0.4 * (TAU * t / (beat_len * 8.0)).sin();
        let pad: f32 = CHORDS[chord].iter().map(|&f| (TAU * f * t).sin()).sum();
        out += 0.05 * swell * pad;

        // Sixteenth-note arpeggio two octaves up — gives treble something to do.
        let sixteenth = beat_len * 0.25;
        let step = (t / sixteenth) as usize;
        let st = t % sixteenth;
        let note = CHORDS[chord][step % 3] * 4.0;
        out += 0.07 * (-st * 30.0).exp() * (TAU * note * t).sin();

        // Gentle saturation keeps peaks musical.
        (out * 0.8).tanh()
    }
}
