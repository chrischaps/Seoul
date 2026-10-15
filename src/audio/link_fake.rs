//! A stand-in Soba: the `--synth` track's structure (124 BPM, a kick-less
//! breakdown every eighth bar) played as Soba link lanes. `--synth` sends it
//! over real UDP, `--record` folds it in directly, and
//! `examples/link_send.rs` runs it against loopback music.
//!
//! Depends on std and rosc only, so the example can include it by path.

use std::f64::consts::TAU;
use std::net::UdpSocket;
use std::thread;
use std::time::{Duration, Instant};

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};

/// Matches `audio/synth.rs`.
pub const BPM: f64 = 124.0;
/// Soba's send rate: one bundle every 5 ms.
pub const TICK: f64 = 0.005;
/// How long a trigger gate stays high.
const TRIGGER: f64 = 0.03;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// A cheap hash to 0..1, for sample-and-hold.
fn hash(n: u64) -> f64 {
    let mut x = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Every lane as it stands `t` seconds into the track.
pub fn messages(t: f64) -> Vec<OscMessage> {
    let beat_len = 60.0 / BPM;
    let beats = t / beat_len;
    let beat = beats.floor();
    let phase = beats - beat;
    let bt = phase * beat_len; // seconds since the beat
    let beat_n = beat as u64;
    let bar = beat_n / 4;
    let beat_in_bar = beat_n % 4;
    let breakdown = bar % 8 == 7;
    let bar_phase = (beats / 4.0).fract();
    let eighth = beat_len * 0.5;
    let eighth_n = (t / eighth) as u64;
    let since_eighth = t - eighth_n as f64 * eighth;

    let cv = [
        0.5 - 0.5 * (TAU * beats / 16.0).cos(),          // 1: 4-bar swell, 0..1
        (TAU * bar_phase).sin(),                         // 2: 1-bar bipolar LFO
        bar_phase,                                       // 3: bar ramp
        if breakdown { 1.0 } else { 0.0 },               // 4: breakdown flag
        0.5 + 0.5 * (TAU * beats / 32.0 + 1.0).sin(),    // 5: 8-bar LFO
        (-bt * 8.0).exp(),                               // 6: per-beat envelope
        (TAU * 0.13 * t).sin(),                          // 7: free-running LFO, unsynced
        hash(beat_n),                                    // 8: sample-and-hold per beat
    ];
    let gates = [
        !breakdown && bt < TRIGGER,                          // 1: kick
        beat_in_bar % 2 == 1 && bt < TRIGGER,                // 2: clap on 2 and 4
        eighth_n % 2 == 1 && since_eighth < TRIGGER,         // 3: off-beat hats
        beat_in_bar == 0,                                    // 4: held through each downbeat
    ];

    let mut out = Vec::with_capacity(14);
    for (i, v) in cv.iter().enumerate() {
        out.push(msg(&format!("/soba/cv/{}", i + 1), vec![OscType::Float(*v as f32)]));
    }
    for (i, g) in gates.iter().enumerate() {
        out.push(msg(&format!("/soba/gate/{}", i + 1), vec![OscType::Int(*g as i32)]));
    }
    out.push(msg(
        "/soba/clock",
        vec![
            OscType::Float(BPM as f32),
            OscType::Float(phase as f32),
            OscType::Int(beat_n as i32),
            OscType::Int(bar as i32),
        ],
    ));
    out.push(msg("/soba/transport", vec![OscType::Int(1)]));
    out
}

/// One tick's bundle, as Soba sends it.
pub fn bundle(t: f64) -> OscPacket {
    OscPacket::Bundle(OscBundle {
        timetag: OscTime::from((0, 1)), // "immediately"
        content: messages(t).into_iter().map(OscPacket::Message).collect(),
    })
}

/// Send to `127.0.0.1:port` at Soba's rate until the process exits, with a
/// hello once a second. Times from `start`.
pub fn run(port: u16, start: Instant, version: &str) -> std::io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.connect(("127.0.0.1", port))?;
    let hello = OscPacket::Message(msg("/soba/hello", vec![OscType::String(version.into())]));
    let hello = rosc::encoder::encode(&hello).expect("encode hello");
    let mut tick: u64 = 0;
    loop {
        let t = tick as f64 * TICK;
        if tick.is_multiple_of(200) {
            // Nobody listening yet is fine; keep sending.
            let _ = socket.send(&hello);
        }
        let bytes = rosc::encoder::encode(&bundle(t)).expect("encode bundle");
        let _ = socket.send(&bytes);
        tick += 1;
        let due = start + Duration::from_secs_f64(tick as f64 * TICK);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    }
}
