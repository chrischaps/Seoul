//! The Soba link: control lanes, gates and an authoritative clock arriving
//! as OSC over localhost UDP (protocol in `docs/LINK.md`).
//!
//! A receiver thread folds every message into a [`LinkState`] and sends a
//! timestamped copy per packet to the render thread. There, [`Link`] keeps a
//! couple of seconds of those snapshots and samples the one from
//! `delay` ago, so `[link] delay_ms` shifts values, gate edges and the clock
//! together to line up with loopback audio, which arrives later than UDP.
//!
//! Times are seconds since the link's epoch (`f64`), not `Instant`s, so
//! `--record` can drive the same code from its simulated clock.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rosc::{OscMessage, OscPacket, OscType};
use tracing::{debug, info, warn};

use crate::audio::AudioFeatures;

pub const DEFAULT_PORT: u16 = 9137;
pub const CV_LANES: usize = 8;
pub const GATE_LANES: usize = 4;
/// The link is live while messages arrive at least this often.
const LIVE_WINDOW: f64 = 0.5;
/// Snapshots kept for `delay` lookups; `delay_ms` is clamped below this.
const HISTORY: f64 = 2.0;
pub const MAX_DELAY_MS: f32 = 1000.0;
/// Same cap as the analysis's `kick_age`: "a long time ago".
const AGE_CAP: f32 = 1000.0;

/// Everything the wire has said so far, as of `last_seen`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkState {
    pub cv: [f32; CV_LANES],
    pub gate: [bool; GATE_LANES],
    /// Rising edges per gate. Counted here, at 200 Hz, so a 5 ms trigger
    /// that falls between two frames still registers.
    pub gate_count: [u32; GATE_LANES],
    /// Time of each gate's last rising edge.
    pub gate_rise: [f64; GATE_LANES],
    pub bpm: f32,
    pub beat_phase: f32,
    pub beat_count: i32,
    pub bar: i32,
    /// When the last `/soba/clock` arrived; `None` until one has.
    pub clock_at: Option<f64>,
    /// Treated as playing until `/soba/transport` says otherwise, so a
    /// sender that never reports transport still drives the clock.
    pub playing: bool,
    pub last_seen: f64,
}

impl Default for LinkState {
    fn default() -> Self {
        Self {
            cv: [0.0; CV_LANES],
            gate: [false; GATE_LANES],
            gate_count: [0; GATE_LANES],
            gate_rise: [f64::NEG_INFINITY; GATE_LANES],
            bpm: 120.0,
            beat_phase: 0.0,
            beat_count: 0,
            bar: 0,
            clock_at: None,
            playing: true,
            last_seen: f64::NEG_INFINITY,
        }
    }
}

/// Lane number `N` of `/soba/<kind>/N` as a 0-based index below `lanes`.
fn lane(addr: &str, prefix: &str, lanes: usize) -> Option<usize> {
    let n: usize = addr.strip_prefix(prefix)?.parse().ok()?;
    (1..=lanes).contains(&n).then(|| n - 1)
}

fn num(arg: Option<&OscType>) -> Option<f64> {
    match arg? {
        OscType::Float(v) => Some(*v as f64),
        OscType::Double(v) => Some(*v),
        OscType::Int(v) => Some(*v as f64),
        OscType::Long(v) => Some(*v as f64),
        OscType::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

impl LinkState {
    /// Fold one message in, received at `now`. Returns false for addresses
    /// or arguments this build doesn't understand; those are ignored so the
    /// protocol can grow.
    pub fn apply(&mut self, msg: &OscMessage, now: f64) -> bool {
        let a = &msg.args;
        let addr = msg.addr.as_str();
        if let Some(i) = lane(addr, "/soba/cv/", CV_LANES) {
            let Some(v) = num(a.first()) else { return false };
            self.cv[i] = v as f32;
        } else if let Some(i) = lane(addr, "/soba/gate/", GATE_LANES) {
            let Some(v) = num(a.first()) else { return false };
            let on = v > 0.5;
            if on && !self.gate[i] {
                self.gate_count[i] += 1;
                self.gate_rise[i] = now;
            }
            self.gate[i] = on;
        } else if addr == "/soba/clock" {
            let (Some(bpm), Some(phase), Some(count), Some(bar)) =
                (num(a.first()), num(a.get(1)), num(a.get(2)), num(a.get(3)))
            else {
                return false;
            };
            self.bpm = bpm as f32;
            self.beat_phase = phase.rem_euclid(1.0) as f32;
            self.beat_count = count as i32;
            self.bar = bar as i32;
            self.clock_at = Some(now);
        } else if addr == "/soba/transport" {
            let Some(v) = num(a.first()) else { return false };
            self.playing = v > 0.5;
        } else if addr == "/soba/hello" {
            // Liveness only; the receiver reads the version itself.
        } else {
            return false;
        }
        true
    }

    /// Write this snapshot into `f` as seen at time `t`. Not live: every
    /// link lane reads 0, gates read as long ago, and the detected clock
    /// stands.
    fn merge_into(&self, f: &mut AudioFeatures, t: f64) {
        if t - self.last_seen > LIVE_WINDOW {
            merge_down(f);
            return;
        }
        f.cv = self.cv;
        for i in 0..GATE_LANES {
            f.gate[i] = if self.gate[i] { 1.0 } else { 0.0 };
            f.gate_count[i] = self.gate_count[i] as f32;
            f.gate_age[i] = ((t - self.gate_rise[i]) as f32).clamp(0.0, AGE_CAP);
        }
        f.link_live = 1.0;
        f.playing = if self.playing { 1.0 } else { 0.0 };

        // Soba's clock replaces detection only while it plays and is fresh:
        // a stopped patch freezes its phase, and a Link Out with no Clock
        // patched in stops sending one.
        if let Some(at) = self.clock_at
            && self.playing
            && t - at <= LIVE_WINDOW
        {
            // Extrapolate between 200 Hz messages so phase moves every frame.
            let beats = self.beat_phase as f64 + self.bpm as f64 / 60.0 * (t - at).max(0.0);
            let wraps = beats.floor();
            f.bpm = self.bpm;
            f.bpm_confidence = 1.0;
            f.beat_phase = (beats - wraps) as f32;
            f.beat_count = (self.beat_count as f64 + wraps) as f32;
            f.bar = self.bar as f32;
        }
    }
}

/// The link's fields with no link: zero lanes, gates long ago.
fn merge_down(f: &mut AudioFeatures) {
    f.cv = [0.0; CV_LANES];
    f.gate = [0.0; GATE_LANES];
    f.gate_age = [AGE_CAP; GATE_LANES];
    f.gate_count = [0.0; GATE_LANES];
    f.link_live = 0.0;
    f.playing = 0.0;
}

/// Recent snapshots, oldest first, for sampling a moment in the past.
#[derive(Default)]
pub struct LinkTimeline {
    history: VecDeque<(f64, LinkState)>,
}

impl LinkTimeline {
    pub fn push(&mut self, t: f64, state: LinkState) {
        self.history.push_back((t, state));
        // Keep one snapshot older than the horizon so a lookup right at the
        // horizon still has the state that was current then.
        while self.history.len() > 1 && self.history[1].0 < t - HISTORY {
            self.history.pop_front();
        }
    }

    /// The newest snapshot taken at or before `t`.
    pub fn sample(&self, t: f64) -> Option<&LinkState> {
        let n = self.history.partition_point(|(at, _)| *at <= t);
        n.checked_sub(1).map(|i| &self.history[i].1)
    }
}

enum Event {
    State(f64, LinkState),
    Hello(String),
}

/// Owns the socket thread; dropping it stops the thread (within ~100 ms)
/// and frees the port.
struct Listener {
    rx: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Binds `127.0.0.1:port` (never all interfaces: no firewall prompt, no
/// remote senders) and starts the receiver thread.
fn spawn_receiver(port: u16, epoch: Instant) -> Result<(Listener, u16)> {
    let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .with_context(|| format!("bind 127.0.0.1:{port}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let bound = socket.local_addr()?.port();
    let (tx, rx) = sync_channel(1024);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    thread::Builder::new()
        .name("seoul-link".into())
        .spawn(move || receive_loop(socket, epoch, tx, &stop_thread))
        .context("spawn link thread")?;
    info!(port = bound, "listening for a Soba link");
    Ok((Listener { rx, stop }, bound))
}

fn receive_loop(socket: UdpSocket, epoch: Instant, tx: SyncSender<Event>, stop: &AtomicBool) {
    // Localhost datagrams aren't bound by an ethernet MTU.
    let mut buf = vec![0u8; 65_536];
    let mut state = LinkState::default();
    let mut live = false;
    let mut version: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let now = epoch.elapsed().as_secs_f64();
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if live && now - state.last_seen > LIVE_WINDOW {
                    live = false;
                    info!("Soba link down");
                }
                continue;
            }
            // Windows reports ICMP "port unreachable" from earlier sends as
            // a recv error (WSAECONNRESET); it means nothing for a listener.
            Err(e) => {
                debug!("link recv: {e}");
                continue;
            }
        };
        let packet = match rosc::decoder::decode_udp(&buf[..n]) {
            Ok((_, p)) => p,
            Err(e) => {
                debug!("link: undecodable packet ({e:?})");
                continue;
            }
        };
        let mut hello = None;
        let mut any = false;
        visit(&packet, &mut |msg| {
            any |= state.apply(msg, now);
            if msg.addr == "/soba/hello"
                && let Some(OscType::String(v)) = msg.args.first()
            {
                hello = Some(v.clone());
            }
            if !msg.addr.starts_with("/soba/") {
                debug!(addr = msg.addr, "link: ignoring non-Soba address");
            }
        });
        if !any {
            continue;
        }
        state.last_seen = now;
        if !live {
            live = true;
            info!("Soba link up");
        }
        if let Some(v) = hello
            && version.as_ref() != Some(&v)
        {
            info!(version = v, "Soba says hello");
            version = Some(v.clone());
            let _ = tx.try_send(Event::Hello(v));
        }
        // A stalled render thread (minimized window) drops snapshots rather
        // than queueing them; the next one carries every lane anyway.
        let _ = tx.try_send(Event::State(now, state));
    }
}

/// Every message in a packet, depth first through bundles.
fn visit(packet: &OscPacket, f: &mut impl FnMut(&OscMessage)) {
    match packet {
        OscPacket::Message(m) => f(m),
        OscPacket::Bundle(b) => b.content.iter().for_each(|p| visit(p, f)),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkSettings {
    pub enabled: bool,
    pub port: u16,
    pub delay_ms: f32,
}

impl Default for LinkSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_PORT,
            delay_ms: 0.0,
        }
    }
}

enum Status {
    Off,
    Listening(u16),
    Busy(u16),
    /// `--record` feeds the timeline itself.
    Offline,
}

/// The render thread's end of the link.
pub struct Link {
    epoch: Instant,
    settings: LinkSettings,
    receiver: Option<Listener>,
    status: Status,
    timeline: LinkTimeline,
    version: Option<String>,
}

impl Link {
    /// Listen as `settings` say. A busy port is reported, not fatal.
    pub fn new(settings: LinkSettings) -> Self {
        let mut link = Self {
            epoch: Instant::now(),
            settings: LinkSettings {
                enabled: false,
                ..settings.clone()
            },
            receiver: None,
            status: Status::Off,
            timeline: LinkTimeline::default(),
            version: None,
        };
        link.configure(settings);
        link
    }

    /// No socket: the caller pushes snapshots on its own clock (`--record`).
    pub fn offline() -> Self {
        Self {
            epoch: Instant::now(),
            settings: LinkSettings::default(),
            receiver: None,
            status: Status::Offline,
            timeline: LinkTimeline::default(),
            version: None,
        }
    }

    /// Apply new settings; rebinds only if `enabled` or `port` changed.
    pub fn configure(&mut self, settings: LinkSettings) {
        if matches!(self.status, Status::Offline) {
            return;
        }
        let rebind = settings.enabled != self.settings.enabled || settings.port != self.settings.port;
        self.settings = settings;
        if !rebind {
            return;
        }
        // Free the old port before binding the new one.
        self.receiver = None;
        self.timeline = LinkTimeline::default();
        self.version = None;
        self.status = Status::Off;
        if !self.settings.enabled {
            return;
        }
        let port = self.settings.port;
        // The old thread can hold the port for up to one read timeout.
        for attempt in 0..3 {
            match spawn_receiver(port, self.epoch) {
                Ok((r, bound)) => {
                    self.receiver = Some(r);
                    self.status = Status::Listening(bound);
                    return;
                }
                Err(e) if attempt == 2 => {
                    warn!("Soba link disabled: {e:#} (is another Seoul running?)");
                    self.status = Status::Busy(port);
                }
                Err(_) => thread::sleep(Duration::from_millis(120)),
            }
        }
    }

    pub fn port(&self) -> Option<u16> {
        match self.status {
            Status::Listening(p) => Some(p),
            _ => None,
        }
    }

    /// Seconds on the link clock.
    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Record a snapshot taken at `t` (offline use).
    pub fn push(&mut self, t: f64, state: LinkState) {
        self.timeline.push(t, state);
    }

    /// Merge the link as it stood `delay_ms` before `now` into `f`.
    pub fn apply(&mut self, f: &mut AudioFeatures, now: f64) {
        if let Some(r) = &self.receiver {
            loop {
                match r.rx.try_recv() {
                    Ok(Event::State(t, s)) => self.timeline.push(t, s),
                    Ok(Event::Hello(v)) => self.version = Some(v),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
        }
        let delay = match self.status {
            Status::Offline => 0.0,
            _ => self.settings.delay_ms.clamp(0.0, MAX_DELAY_MS) as f64 / 1000.0,
        };
        let t = now - delay;
        match self.timeline.sample(t) {
            Some(s) => s.merge_into(f, t),
            None => merge_down(f),
        }
    }

    /// One line for the F1 panel.
    pub fn status_line(&self, f: &AudioFeatures) -> String {
        match self.status {
            Status::Off => "off".into(),
            Status::Busy(p) => format!("port {p} busy"),
            Status::Offline | Status::Listening(_) if f.link_live > 0.5 => {
                let who = self.version.as_deref().unwrap_or("soba");
                let play = if f.playing > 0.5 { "playing" } else { "stopped" };
                let delay = match self.status {
                    Status::Listening(_) if self.settings.delay_ms > 0.0 => {
                        format!(" · +{:.0} ms", self.settings.delay_ms.min(MAX_DELAY_MS))
                    }
                    _ => String::new(),
                };
                format!("{who} · {play}{delay}")
            }
            Status::Listening(p) => format!("waiting on :{p}"),
            Status::Offline => "waiting".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
        OscMessage {
            addr: addr.into(),
            args,
        }
    }

    fn gate(state: &mut LinkState, on: i32, t: f64) {
        state.apply(&msg("/soba/gate/2", vec![OscType::Int(on)]), t);
    }

    #[test]
    fn applies_lanes_and_ignores_the_unknown() {
        let mut s = LinkState::default();
        assert!(s.apply(&msg("/soba/cv/1", vec![OscType::Float(0.25)]), 0.0));
        assert!(s.apply(&msg("/soba/cv/8", vec![OscType::Double(-0.5)]), 0.0));
        assert!(s.apply(&msg("/soba/transport", vec![OscType::Int(0)]), 0.0));
        assert_eq!((s.cv[0], s.cv[7], s.playing), (0.25, -0.5, false));
        assert!(!s.apply(&msg("/soba/cv/9", vec![OscType::Float(1.0)]), 0.0));
        assert!(!s.apply(&msg("/soba/cv/0", vec![OscType::Float(1.0)]), 0.0));
        assert!(!s.apply(&msg("/soba/cv/1", vec![OscType::String("x".into())]), 0.0));
        assert!(!s.apply(&msg("/soba/sparkle", vec![]), 0.0));
        assert!(!s.apply(&msg("/soba/clock", vec![OscType::Float(120.0)]), 0.0));
        assert_eq!(s.cv[0], 0.25);
    }

    #[test]
    fn counts_rising_edges() {
        let mut s = LinkState::default();
        for (i, on) in [0, 1, 1, 0, 1].into_iter().enumerate() {
            gate(&mut s, on, i as f64);
        }
        assert_eq!(s.gate_count[1], 2);
        assert_eq!(s.gate_rise[1], 4.0);
        assert!(s.gate[1]);
    }

    #[test]
    fn samples_the_past_for_delay() {
        let mut tl = LinkTimeline::default();
        let mut s = LinkState::default();
        for i in 0..400 {
            let t = i as f64 * 0.005;
            s.cv[0] = t as f32;
            s.last_seen = t;
            tl.push(t, s);
        }
        assert_eq!(tl.sample(1.0).unwrap().cv[0], 1.0);
        assert_eq!(tl.sample(1.0024).unwrap().cv[0], 1.0);
        assert!(tl.sample(-0.1).is_none());
        // Pruned to about HISTORY.
        for i in 400..1200 {
            let t = i as f64 * 0.005;
            s.last_seen = t;
            tl.push(t, s);
        }
        assert!(tl.history.len() <= 402, "{}", tl.history.len());
    }

    #[test]
    fn liveness_and_gate_ages() {
        let mut s = LinkState::default();
        gate(&mut s, 1, 10.0);
        s.cv[2] = 0.7;
        s.last_seen = 10.0;
        let mut f = AudioFeatures::default();
        s.merge_into(&mut f, 10.25);
        assert_eq!(f.link_live, 1.0);
        assert_eq!(f.cv[2], 0.7);
        assert_eq!(f.gate[1], 1.0);
        assert_eq!(f.gate_count[1], 1.0);
        assert!((f.gate_age[1] - 0.25).abs() < 1e-6);
        assert_eq!(f.gate_age[0], AGE_CAP);
        s.merge_into(&mut f, 10.6);
        assert_eq!(f.link_live, 0.0);
        assert_eq!((f.cv[2], f.gate[1], f.gate_age[1]), (0.0, 0.0, AGE_CAP));
    }

    fn clocked(playing: bool) -> LinkState {
        let mut s = LinkState::default();
        s.apply(
            &msg(
                "/soba/clock",
                vec![OscType::Float(120.0), OscType::Float(0.9), OscType::Int(7), OscType::Int(1)],
            ),
            5.0,
        );
        s.apply(&msg("/soba/transport", vec![OscType::Int(playing as i32)]), 5.0);
        s.last_seen = 5.0;
        s
    }

    #[test]
    fn clock_overrides_and_extrapolates_across_a_beat() {
        let mut f = AudioFeatures {
            bpm_confidence: 0.2,
            ..Default::default()
        };
        // 120 bpm: 0.1 s is 0.2 beats, so phase 0.9 wraps to 0.1 on beat 8.
        clocked(true).merge_into(&mut f, 5.1);
        assert_eq!((f.bpm, f.bpm_confidence, f.bar), (120.0, 1.0, 1.0));
        assert!((f.beat_phase - 0.1).abs() < 1e-5, "{}", f.beat_phase);
        assert_eq!(f.beat_count, 8.0);
    }

    #[test]
    fn stopped_transport_leaves_detection_alone() {
        let mut f = AudioFeatures {
            bpm: 97.0,
            beat_phase: 0.3,
            ..Default::default()
        };
        clocked(false).merge_into(&mut f, 5.1);
        assert_eq!((f.link_live, f.playing), (1.0, 0.0));
        assert_eq!((f.bpm, f.beat_phase), (97.0, 0.3));
    }

    #[test]
    fn stand_in_soba_plays_the_synth_track() {
        use crate::audio::link_fake::{BPM, TICK, messages};
        // Eight bars at the 200 Hz send rate; bar 8 is the kick-less breakdown.
        let end = 32.0 * 60.0 / BPM;
        let mut s = LinkState::default();
        let mut tick = 0u64;
        while (tick as f64 * TICK) < end - 1e-9 {
            let t = tick as f64 * TICK;
            for m in messages(t) {
                assert!(s.apply(&m, t), "{}", m.addr);
            }
            tick += 1;
        }
        assert_eq!(s.gate_count, [28, 16, 32, 8]);
        assert_eq!((s.bpm, s.beat_count, s.bar), (124.0, 31, 7));
        assert!(s.cv[3] == 1.0, "breakdown flag");
    }

    #[test]
    fn receives_a_bundle_over_udp() {
        let epoch = Instant::now();
        let (r, port) = spawn_receiver(0, epoch).unwrap();
        let packet = OscPacket::Bundle(rosc::OscBundle {
            timetag: rosc::OscTime::from((0, 1)),
            content: vec![
                OscPacket::Message(msg("/soba/cv/3", vec![OscType::Float(0.5)])),
                OscPacket::Message(msg("/soba/gate/1", vec![OscType::Int(1)])),
                OscPacket::Message(msg("/soba/hello", vec![OscType::String("test 1".into())])),
            ],
        });
        let bytes = rosc::encoder::encode(&packet).unwrap();
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        tx.send_to(&bytes, ("127.0.0.1", port)).unwrap();
        let mut hello = None;
        let state = loop {
            match r.rx.recv_timeout(Duration::from_secs(2)).expect("no snapshot") {
                Event::Hello(v) => hello = Some(v),
                Event::State(_, s) => break s,
            }
        };
        assert_eq!(hello.as_deref(), Some("test 1"));
        assert_eq!(state.cv[2], 0.5);
        assert_eq!(state.gate_count[0], 1);
    }
}
