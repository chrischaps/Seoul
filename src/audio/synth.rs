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

pub fn spawn_synth(sink: Sender<AudioSource>) {
    let (mut producer, consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
    let _ = sink.send(AudioSource {
        consumer,
        sample_rate: SAMPLE_RATE,
    });
    info!(bpm = BPM, "synth test signal running");

    thread::Builder::new()
        .name("seoul-synth".into())
        .spawn(move || {
            let mut synth = Synth::default();
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
