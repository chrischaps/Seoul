//! Frame mode: Seoul as a picture on the wall.
//!
//! A [`MoodTracker`] reads the analysis and decides what the room is doing —
//! resting (nothing heard), listening (sound without a groove: talk, a
//! cough, a door) or playing (music). [`FrameMode`] turns that and the time
//! of day into a [`Look`] — how fast the picture moves, how bright it is,
//! how strongly it follows the sound, how warm its light is — and eases
//! every part of it, so the frame breathes between states instead of
//! switching. The renderer applies speed, exposure and warmth; [`apply`]
//! scales what presets hear.

use crate::audio::AudioFeatures;

/// What the room is doing, as frame mode reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Resting,
    Listening,
    Playing,
}

impl Mood {
    pub fn label(self) -> &'static str {
        match self {
            Mood::Resting => "resting",
            Mood::Listening => "listening",
            Mood::Playing => "playing",
        }
    }
}

// Mood detection. `volume` is AGC'd (and, for a room mic, already zero for
// the room's own noise), so a fixed level reads as "something is sounding".
const SOUND_LEVEL: f32 = 0.15;
/// Seconds of nothing before the frame rests.
const REST_AFTER: f32 = 4.0;
/// `sustain` is the share of roughly the last `SUSTAIN_TAU` seconds with
/// sound. Talk is full of gaps; music, even without drums, mostly isn't.
const SUSTAIN_TAU: f32 = 8.0;
const SUSTAIN_HELD: f32 = 0.9;
/// A groove: confident tempo with a beat in the last `GROOVE_BEAT_GAP` s.
const GROOVE_CONFIDENCE: f32 = 0.35;
const GROOVE_BEAT_GAP: f32 = 1.5;
const GROOVE_SUSTAIN: f32 = 0.5;
/// Talk stops every few seconds, between phrases; a record doesn't stop at
/// all. Sound that runs `UNBROKEN_PLAY` seconds without a gap (at least
/// `GAP_MIN` s below `SOUND_LEVEL`) is music.
const GAP_MIN: f32 = 0.15;
const UNBROKEN_PLAY: f32 = 4.0;
/// Music must hold this long to start playing, and be gone this long to stop.
const PLAY_AFTER: f32 = 2.0;
const STOP_AFTER: f32 = 5.0;

/// Reads the room from the analysis, one frame at a time. Pure, so tests
/// can drive it with an analyzer and a scripted room.
pub struct MoodTracker {
    mood: Mood,
    sustain: f32,
    quiet_for: f32,
    /// Seconds of sound since the last gap.
    unbroken: f32,
    musical_for: f32,
    unmusical_for: f32,
    last_beat_count: f32,
    since_beat: f32,
}

impl Default for MoodTracker {
    fn default() -> Self {
        Self {
            mood: Mood::Resting,
            sustain: 0.0,
            quiet_for: REST_AFTER,
            unbroken: 0.0,
            musical_for: 0.0,
            unmusical_for: STOP_AFTER,
            last_beat_count: 0.0,
            since_beat: f32::INFINITY,
        }
    }
}

impl MoodTracker {
    pub fn mood(&self) -> Mood {
        self.mood
    }

    pub fn update(&mut self, f: &AudioFeatures, dt: f32) -> Mood {
        let sounding = f.volume > SOUND_LEVEL;
        self.sustain += (f32::from(u8::from(sounding)) - self.sustain) * (1.0 - (-dt / SUSTAIN_TAU).exp());
        self.quiet_for = if sounding { 0.0 } else { self.quiet_for + dt };
        self.unbroken = if self.quiet_for >= GAP_MIN { 0.0 } else { self.unbroken + dt };
        if f.beat_count != self.last_beat_count {
            self.last_beat_count = f.beat_count;
            self.since_beat = 0.0;
        } else {
            self.since_beat += dt;
        }

        let groove =
            f.bpm_confidence >= GROOVE_CONFIDENCE && self.since_beat < GROOVE_BEAT_GAP && self.sustain > GROOVE_SUSTAIN;
        let held = self.sustain >= SUSTAIN_HELD || self.unbroken >= UNBROKEN_PLAY;
        // A live Soba link that's playing is music by definition.
        let link = f.link_live > 0.5 && f.playing > 0.5;
        if groove || held || link {
            self.musical_for += dt;
            self.unmusical_for = 0.0;
        } else {
            self.unmusical_for += dt;
            self.musical_for = 0.0;
        }

        self.mood = if link
            || self.musical_for >= PLAY_AFTER
            || (self.mood == Mood::Playing && self.unmusical_for < STOP_AFTER)
        {
            Mood::Playing
        } else if self.quiet_for < REST_AFTER {
            Mood::Listening
        } else {
            Mood::Resting
        };
        self.mood
    }
}

/// How the frame looks this frame. Every field eases; none jumps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Scene time per real second: presets, feedback, particles and
    /// transitions all slow together.
    pub speed: f32,
    /// Multiplies the preset's own exposure.
    pub exposure: f32,
    /// How strongly presets hear the music (bands, spectrum, waveform).
    pub reactivity: f32,
    /// How strongly they hear beats and drum hits.
    pub beats: f32,
    /// 0 = day, 1 = the night grade (warmer light).
    pub warmth: f32,
}

impl Look {
    const RESTING: Look = Look { speed: 0.25, exposure: 0.4, reactivity: 0.0, beats: 0.0, warmth: 0.0 };
    const LISTENING: Look = Look { speed: 0.5, exposure: 0.65, reactivity: 0.35, beats: 0.0, warmth: 0.0 };
    const PLAYING: Look = Look { speed: 1.0, exposure: 1.0, reactivity: 1.0, beats: 1.0, warmth: 0.0 };

    fn for_mood(mood: Mood) -> Look {
        match mood {
            Mood::Resting => Look::RESTING,
            Mood::Listening => Look::LISTENING,
            Mood::Playing => Look::PLAYING,
        }
    }
}

/// The picture wakes quickly and settles slowly.
const WAKE_TAU: f32 = 1.5;
const SETTLE_TAU: f32 = 6.0;
/// At night the frame is this much dimmer, and its grade turns warm over
/// `NIGHT_TAU` seconds.
const NIGHT_EXPOSURE: f32 = 0.75;
const NIGHT_TAU: f32 = 60.0;
/// Falling asleep fades to black with this time constant (dark in about
/// 40 s); waking is quicker.
const SLEEP_TAU: f32 = 10.0;
const SLEEP_WAKE_TAU: f32 = 3.0;

/// Frame mode's settings, from `[frame]` in `seoul.toml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSettings {
    /// Minutes past midnight: the night grade starts at `night`, the screen
    /// sleeps from `sleep` until `wake`.
    pub night: f32,
    pub sleep: f32,
    pub wake: f32,
}

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            night: 20.0 * 60.0,
            sleep: 23.0 * 60.0,
            wake: 7.0 * 60.0,
        }
    }
}

/// Where the time of day comes from.
#[derive(Debug, Clone, Copy)]
pub enum Clock {
    /// The computer's local time.
    Local,
    /// Pretend: starts at these minutes past midnight and runs with the
    /// frames (`--clock`, and recordings).
    Fixed(f32),
}

impl Clock {
    fn minutes(&self) -> f32 {
        match *self {
            Clock::Local => {
                use chrono::Timelike;
                let t = chrono::Local::now();
                (t.hour() * 60 + t.minute()) as f32 + t.second() as f32 / 60.0
            }
            Clock::Fixed(m) => m,
        }
    }

    fn advance(&mut self, dt: f32) {
        if let Clock::Fixed(m) = self {
            *m = (*m + dt / 60.0).rem_euclid(24.0 * 60.0);
        }
    }
}

/// True if `now` is in [from, to), on a 24-hour clock that may wrap midnight.
fn within(now: f32, from: f32, to: f32) -> bool {
    if from <= to { now >= from && now < to } else { now >= from || now < to }
}

/// "HH:MM" → minutes past midnight.
pub fn parse_clock(s: &str) -> Option<f32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h * 60 + m) as f32)
}

pub struct FrameMode {
    pub settings: FrameSettings,
    clock: Clock,
    tracker: MoodTracker,
    look: Look,
    /// 0 = awake, 1 = asleep (eased).
    sleep: f32,
    /// The first update snaps night and sleep to the hour instead of
    /// easing: a frame switched on at 2 am starts dark, not glowing.
    started: bool,
}

impl FrameMode {
    pub fn new(settings: FrameSettings, clock: Clock) -> Self {
        Self {
            settings,
            clock,
            tracker: MoodTracker::default(),
            look: Look::RESTING,
            sleep: 0.0,
            started: false,
        }
    }

    pub fn mood(&self) -> Mood {
        self.tracker.mood()
    }

    pub fn is_night(&self) -> bool {
        within(self.clock.minutes(), self.settings.night, self.settings.wake)
    }

    /// In the sleep window. Music still wakes the frame (see `update`).
    pub fn is_sleep_time(&self) -> bool {
        within(self.clock.minutes(), self.settings.sleep, self.settings.wake)
    }

    /// Fully faded out: the caller can stop drawing at full rate.
    pub fn is_dark(&self) -> bool {
        self.sleep > 0.995
    }

    /// Read the room from `features` (as analysed, before [`apply`]) and
    /// ease toward the matching look. `dt` is real seconds.
    pub fn update(&mut self, features: &AudioFeatures, dt: f32) -> Look {
        self.clock.advance(dt);
        let mood = self.tracker.update(features, dt);

        let mut target = Look::for_mood(mood);
        let night = self.is_night();
        target.warmth = if night { 1.0 } else { 0.0 };
        if night {
            target.exposure *= NIGHT_EXPOSURE;
        }
        // A record played at midnight wakes the frame; talk doesn't.
        let asleep = self.is_sleep_time() && mood != Mood::Playing;
        let tau = if asleep { SLEEP_TAU } else { SLEEP_WAKE_TAU };
        self.sleep = ease(self.sleep, if asleep { 1.0 } else { 0.0 }, dt, tau, tau);
        if !std::mem::replace(&mut self.started, true) {
            self.sleep = if asleep { 1.0 } else { 0.0 };
            self.look.warmth = target.warmth;
            self.look.exposure = target.exposure;
        }

        let l = &mut self.look;
        l.speed = ease(l.speed, target.speed, dt, WAKE_TAU, SETTLE_TAU);
        l.exposure = ease(l.exposure, target.exposure, dt, WAKE_TAU, SETTLE_TAU);
        l.reactivity = ease(l.reactivity, target.reactivity, dt, WAKE_TAU, SETTLE_TAU);
        l.beats = ease(l.beats, target.beats, dt, WAKE_TAU, SETTLE_TAU);
        l.warmth = ease(l.warmth, target.warmth, dt, NIGHT_TAU, NIGHT_TAU);

        let mut out = *l;
        out.exposure *= 1.0 - self.sleep;
        out
    }

    /// A one-line summary for the stats panel.
    pub fn status(&self) -> String {
        let time = if self.is_sleep_time() {
            if self.is_dark() { "asleep" } else { "sleep window" }
        } else if self.is_night() {
            "night"
        } else {
            "day"
        };
        let m = self.clock.minutes();
        format!("{} · {time} · {:02}:{:02}", self.mood().label(), (m / 60.0) as u32, (m % 60.0) as u32)
    }
}

/// One-pole ease with separate up/down time constants (seconds).
fn ease(cur: f32, target: f32, dt: f32, up: f32, down: f32) -> f32 {
    let tau = if target > cur { up } else { down };
    cur + (target - cur) * (1.0 - (-dt / tau).exp())
}

/// What presets hear: scale the music by `reactivity` and beats by
/// `beats`. With beats faded out, drum ages read "long ago" so presets
/// that flash on a fresh hit stay still.
pub fn apply(look: &Look, f: &mut AudioFeatures) {
    let r = look.reactivity;
    for v in [
        &mut f.bass,
        &mut f.mid,
        &mut f.treble,
        &mut f.bass_att,
        &mut f.mid_att,
        &mut f.treble_att,
        &mut f.volume,
    ] {
        *v *= r;
    }
    let b = r * look.beats;
    for v in [&mut f.beat, &mut f.kick, &mut f.snare, &mut f.hat] {
        *v *= b;
    }
    if look.beats < 0.5 {
        // Matches the analyzer's "before the first hit" value.
        for age in [&mut f.kick_age, &mut f.snare_age, &mut f.hat_age] {
            *age = age.max(1000.0);
        }
    }
    for s in f.spectrum.iter_mut() {
        *s *= r;
    }
    for w in f.waveform.iter_mut() {
        *w *= r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::analysis::Analyzer;
    use crate::audio::synth::{SAMPLE_RATE, Scene, TestTrack};

    /// Run the scripted evening through the room-tuned analyzer and the
    /// mood tracker at 60 frames a second; return (time, scene, mood) per
    /// frame and the tracker's evidence for the report.
    fn evening(seconds: f32) -> Vec<(f32, Scene, Mood, f32, f32, f32)> {
        let mut track = TestTrack::evening();
        let mut a = Analyzer::new(SAMPLE_RATE);
        a.set_room(true);
        let mut m = MoodTracker::default();
        let per_frame = SAMPLE_RATE as usize / 60;
        let mut out = Vec::new();
        for i in 0..(seconds * 60.0) as usize {
            let chunk: Vec<f32> = (0..per_frame).map(|_| track.next_sample()).collect();
            a.push(&chunk);
            let t = (i + 1) as f32 / 60.0;
            let f = a.features();
            let mood = m.update(f, 1.0 / 60.0);
            out.push((t, Scene::at(t), mood, m.sustain, m.unbroken, f.volume));
        }
        out
    }

    /// `cargo test --release mood_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mood_report() {
        let run = evening(100.0);
        let mut last = None;
        for &(t, scene, mood, ..) in &run {
            if last != Some(mood) {
                println!("{t:6.2} s  {scene:?} → {}", mood.label());
                last = Some(mood);
            }
        }
        for &(t, scene, mood, sustain, unbroken, vol) in run.iter().step_by(60) {
            println!("{t:5.0} {scene:?}  {:9} sustain {sustain:.2} unbroken {unbroken:5.2} vol {vol:.2}", mood.label());
        }
        let talk_max = run.iter().filter(|r| r.1 == Scene::Talk).map(|r| r.4).fold(0.0, f32::max);
        println!("longest unbroken run during talk: {talk_max:.2} s");
    }

    #[test]
    fn an_evening_reads_right() {
        // Through the simulated room: rests while it's empty, listens (and
        // never plays) through 20 s of talk, plays within 10 s of the record
        // going on and for the rest of it, and rests within 10 s of it ending.
        let run = evening(100.0);
        let moods = |from: f32, to: f32| run.iter().filter(move |r| r.0 >= from && r.0 < to).map(|r| r.2);
        assert!(moods(0.0, 12.0).all(|m| m == Mood::Resting));
        let talk: Vec<Mood> = moods(13.0, 32.0).collect();
        assert!(!talk.contains(&Mood::Playing), "talk read as music");
        let listening = talk.iter().filter(|&&m| m == Mood::Listening).count();
        assert!(listening as f32 > 0.9 * talk.len() as f32, "talk mostly unheard");
        assert!(moods(48.0, 80.0).all(|m| m == Mood::Playing), "record not playing throughout");
        assert!(moods(90.0, 100.0).all(|m| m == Mood::Resting));
    }

    #[test]
    fn a_playing_link_is_music() {
        let mut m = MoodTracker::default();
        let mut f = AudioFeatures::default();
        f.link_live = 1.0;
        f.playing = 1.0;
        assert_eq!(m.update(&f, 1.0 / 60.0), Mood::Playing);
        f.playing = 0.0;
        for _ in 0..(10 * 60) {
            m.update(&f, 1.0 / 60.0);
        }
        assert_eq!(m.mood(), Mood::Resting);
    }

    #[test]
    fn clock_windows_wrap_midnight() {
        let s = FrameSettings::default();
        assert!(within(23.5 * 60.0, s.sleep, s.wake));
        assert!(within(3.0 * 60.0, s.sleep, s.wake));
        assert!(!within(7.0 * 60.0, s.sleep, s.wake));
        assert!(within(21.0 * 60.0, s.night, s.wake));
        assert!(!within(12.0 * 60.0, s.night, s.wake));
        assert_eq!(parse_clock("07:30"), Some(450.0));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("7"), None);
    }

    #[test]
    fn fixed_clock_runs_and_wraps() {
        let mut c = Clock::Fixed(23.0 * 60.0 + 59.0);
        c.advance(120.0);
        assert!((c.minutes() - 1.0).abs() < 1e-3, "{}", c.minutes());
    }

    #[test]
    fn apply_scales_music_and_hides_fresh_hits() {
        let mut f = AudioFeatures::default();
        f.bass = 0.8;
        f.beat = 1.0;
        f.kick_age = 0.05;
        f.spectrum[3] = 0.5;
        f.bpm = 124.0;
        apply(&Look::LISTENING, &mut f);
        assert!((f.bass - 0.28).abs() < 1e-6);
        assert_eq!(f.beat, 0.0);
        assert_eq!(f.kick_age, 1000.0);
        assert!((f.spectrum[3] - 0.175).abs() < 1e-6);
        assert_eq!(f.bpm, 124.0, "tempo is untouched");
    }

    #[test]
    fn asleep_unless_playing_and_dark_when_faded() {
        let mut fm = FrameMode::new(FrameSettings::default(), Clock::Fixed(23.5 * 60.0));
        let quiet = AudioFeatures::default();
        let mut look = fm.update(&quiet, 0.0);
        for _ in 0..(150 * 60) {
            look = fm.update(&quiet, 1.0 / 60.0);
        }
        assert!(fm.is_dark() && look.exposure < 0.01, "{look:?}");
        assert!(look.warmth > 0.9);
    }
}
