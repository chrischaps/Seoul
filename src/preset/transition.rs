//! Preset state machine: either Stable on one preset, or Transitioning
//! between two with linear progress.

pub const TRANSITION_DURATION: f32 = 2.0;

#[derive(Debug, Clone, Copy)]
pub enum PresetState {
    Stable { current: usize },
    Transitioning { from: usize, to: usize, progress: f32 },
}

impl PresetState {
    pub fn stable(idx: usize) -> Self {
        PresetState::Stable { current: idx }
    }

    /// The preset we're heading toward (or currently on).
    pub fn destination(self) -> usize {
        match self {
            PresetState::Stable { current } => current,
            PresetState::Transitioning { to, .. } => to,
        }
    }

    /// Begin a new transition to `target`. If we're already transitioning, the
    /// current "destination" becomes the new "from" — so rapid switching
    /// stays responsive without thrashing the interpolation.
    pub fn begin_transition(&mut self, target: usize) {
        let from = self.destination();
        if from == target {
            return;
        }
        *self = PresetState::Transitioning {
            from,
            to: target,
            progress: 0.0,
        };
    }

    /// Advance progress by `dt`. Returns `true` if the transition just
    /// completed (state collapsed to Stable this tick).
    pub fn tick(&mut self, dt: f32) -> bool {
        if let PresetState::Transitioning { to, progress, .. } = self {
            *progress += dt / TRANSITION_DURATION;
            if *progress >= 1.0 {
                *self = PresetState::Stable { current: *to };
                return true;
            }
        }
        false
    }
}

impl PresetState {
    /// Account for preset `idx` being removed from the library: indices above
    /// it shift down. Returns false (and changes nothing) if `idx` is in use.
    pub fn remove_index(&mut self, idx: usize) -> bool {
        let shift = |i: usize| if i > idx { i - 1 } else { i };
        match self {
            PresetState::Stable { current } => {
                if *current == idx {
                    return false;
                }
                *current = shift(*current);
            }
            PresetState::Transitioning { from, to, .. } => {
                if *from == idx || *to == idx {
                    return false;
                }
                *from = shift(*from);
                *to = shift(*to);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_doesnt_advance() {
        let mut s = PresetState::stable(0);
        assert!(!s.tick(1.0));
        assert!(matches!(s, PresetState::Stable { current: 0 }));
    }

    #[test]
    fn transition_progresses() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1);
        assert!(matches!(s, PresetState::Transitioning { from: 0, to: 1, .. }));
        let _ = s.tick(0.5);
        if let PresetState::Transitioning { progress, .. } = s {
            assert!((progress - 0.25).abs() < 1e-6);
        } else {
            panic!("expected transitioning");
        }
    }

    #[test]
    fn transition_completes() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1);
        let completed = s.tick(TRANSITION_DURATION + 0.1);
        assert!(completed);
        assert!(matches!(s, PresetState::Stable { current: 1 }));
    }

    #[test]
    fn switching_mid_transition_uses_destination_as_from() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1);
        let _ = s.tick(TRANSITION_DURATION * 0.5);
        s.begin_transition(2);
        assert!(matches!(s, PresetState::Transitioning { from: 1, to: 2, progress } if progress == 0.0));
    }

    #[test]
    fn removal_shifts_indices_and_protects_active() {
        let mut s = PresetState::stable(3);
        assert!(s.remove_index(1));
        assert!(matches!(s, PresetState::Stable { current: 2 }));
        assert!(!s.remove_index(2));
        let mut t = PresetState::stable(0);
        t.begin_transition(4);
        assert!(t.remove_index(2));
        assert!(matches!(t, PresetState::Transitioning { from: 0, to: 3, .. }));
        assert!(!t.remove_index(0));
    }

    #[test]
    fn no_op_transition_to_same_preset() {
        let mut s = PresetState::stable(0);
        s.begin_transition(0);
        assert!(matches!(s, PresetState::Stable { current: 0 }));
    }
}
