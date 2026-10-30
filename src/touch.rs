//! Touchscreen gestures, for a frame with no keyboard: a tap advances to the
//! next preset, a two-finger tap quits. Only taps count, so a resting hand,
//! a swipe or a long press does nothing. Mouse clicks never reach here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// From the first finger down to the last one up.
const TAP_TIME: Duration = Duration::from_millis(400);
/// How far a finger may drift and still be tapping, in physical pixels.
const TAP_SLOP: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Tap,
    TwoFingerTap,
}

/// Fingers on the glass since the screen was last clear.
#[derive(Default)]
pub struct Touches {
    down: HashMap<u64, (f64, f64)>,
    started: Option<Instant>,
    fingers: usize,
    moved: bool,
}

impl Touches {
    pub fn start(&mut self, id: u64, at: (f64, f64), now: Instant) {
        if self.down.is_empty() {
            *self = Touches { started: Some(now), ..Default::default() };
        }
        self.down.insert(id, at);
        self.fingers = self.fingers.max(self.down.len());
    }

    pub fn moved(&mut self, id: u64, at: (f64, f64)) {
        if let Some(&(x, y)) = self.down.get(&id)
            && (at.0 - x).hypot(at.1 - y) > TAP_SLOP
        {
            self.moved = true;
        }
    }

    /// A finger lifted. When it was the last, says what the touch was.
    pub fn end(&mut self, id: u64, now: Instant) -> Option<Gesture> {
        self.down.remove(&id)?;
        if !self.down.is_empty() || self.moved {
            return None;
        }
        let quick = self.started.is_some_and(|t| now - t <= TAP_TIME);
        match self.fingers {
            1 if quick => Some(Gesture::Tap),
            2 if quick => Some(Gesture::TwoFingerTap),
            _ => None,
        }
    }

    /// The system took the touch (a compositor gesture, say): nothing counts.
    pub fn cancel(&mut self, id: u64) {
        self.down.remove(&id);
        self.moved = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn a_quick_touch_is_a_tap() {
        let t0 = Instant::now();
        let mut t = Touches::default();
        t.start(1, (100.0, 100.0), t0);
        t.moved(1, (110.0, 104.0));
        assert_eq!(t.end(1, ms(t0, 120)), Some(Gesture::Tap));
    }

    #[test]
    fn a_swipe_or_a_long_press_is_not() {
        let t0 = Instant::now();
        let mut t = Touches::default();
        t.start(1, (100.0, 100.0), t0);
        t.moved(1, (300.0, 100.0));
        assert_eq!(t.end(1, ms(t0, 150)), None);
        t.start(2, (100.0, 100.0), ms(t0, 1000));
        assert_eq!(t.end(2, ms(t0, 1900)), None);
    }

    #[test]
    fn two_fingers_together_are_one_gesture() {
        let t0 = Instant::now();
        let mut t = Touches::default();
        t.start(1, (100.0, 100.0), t0);
        t.start(2, (300.0, 120.0), ms(t0, 40));
        assert_eq!(t.end(1, ms(t0, 150)), None, "one finger is still down");
        assert_eq!(t.end(2, ms(t0, 190)), Some(Gesture::TwoFingerTap));
    }

    #[test]
    fn a_tap_after_a_swipe_still_counts() {
        let t0 = Instant::now();
        let mut t = Touches::default();
        t.start(1, (0.0, 0.0), t0);
        t.moved(1, (200.0, 0.0));
        assert_eq!(t.end(1, ms(t0, 100)), None);
        t.start(1, (50.0, 50.0), ms(t0, 500));
        assert_eq!(t.end(1, ms(t0, 600)), Some(Gesture::Tap));
    }

    #[test]
    fn three_fingers_or_a_cancel_do_nothing() {
        let t0 = Instant::now();
        let mut t = Touches::default();
        for id in 1..=3 {
            t.start(id, (id as f64 * 100.0, 0.0), t0);
        }
        assert_eq!(t.end(1, ms(t0, 100)), None);
        assert_eq!(t.end(2, ms(t0, 100)), None);
        assert_eq!(t.end(3, ms(t0, 100)), None);
        t.start(4, (0.0, 0.0), ms(t0, 500));
        t.cancel(4);
        assert_eq!(t.end(4, ms(t0, 550)), None);
    }
}
