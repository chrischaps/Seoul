//! Preset state machine: either Stable on one preset, or Transitioning
//! between two with a linear `progress` that callers ease via [`ease`].

use rand::RngExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionStyle {
    /// Uniform blend.
    Crossfade,
    /// Per-pixel noise threshold — the new preset grows in like frost.
    Dissolve,
    /// A soft circle opening from the center.
    Radial,
    /// A clock-hand sweep.
    Clock,
    /// Crossfade while the outgoing preset's feedback accelerates away.
    Zoom,
}

impl TransitionStyle {
    pub const ALL: [Self; 5] = [Self::Crossfade, Self::Dissolve, Self::Radial, Self::Clock, Self::Zoom];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "crossfade" | "fade" => Self::Crossfade,
            "dissolve" => Self::Dissolve,
            "radial" | "radial-wipe" => Self::Radial,
            "clock" | "clock-wipe" => Self::Clock,
            "zoom" => Self::Zoom,
            _ => return None,
        })
    }

    pub fn random() -> Self {
        Self::ALL[rand::rng().random_range(0..Self::ALL.len())]
    }

    pub fn as_f32(self) -> f32 {
        self as u8 as f32
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PresetState {
    Stable {
        current: usize,
    },
    Transitioning {
        from: usize,
        to: usize,
        /// Linear 0→1; ease with [`ease`] before use.
        progress: f32,
        style: TransitionStyle,
        /// Per-transition random seed for noise-based masks.
        seed: f32,
    },
}

/// Smootherstep: zero first and second derivative at both ends, and
/// symmetric (`ease(1 - x) == 1 - ease(x)`), which retargeting relies on.
pub fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
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

    /// Begin a transition to `target`.
    ///
    /// Mid-transition retargets never pop the dominant layer: going back to
    /// `from` reverses in place; otherwise whichever preset currently
    /// carries more weight keeps its exact weight and the minor layer is
    /// swapped for the new target.
    pub fn begin_transition(&mut self, target: usize, style: TransitionStyle, seed: f32) {
        *self = match *self {
            PresetState::Stable { current } if current == target => return,
            PresetState::Stable { current } => PresetState::Transitioning {
                from: current,
                to: target,
                progress: 0.0,
                style,
                seed,
            },
            PresetState::Transitioning { to, .. } if to == target => return,
            PresetState::Transitioning {
                from,
                to,
                progress,
                style: cur_style,
                seed: cur_seed,
            } => {
                if target == from {
                    PresetState::Transitioning {
                        from: to,
                        to: from,
                        progress: 1.0 - progress,
                        style: cur_style,
                        seed: cur_seed,
                    }
                } else if progress < 0.5 {
                    PresetState::Transitioning {
                        from,
                        to: target,
                        progress,
                        style: cur_style,
                        seed: cur_seed,
                    }
                } else {
                    PresetState::Transitioning {
                        from: to,
                        to: target,
                        progress: 1.0 - progress,
                        style: cur_style,
                        seed: cur_seed,
                    }
                }
            }
        };
    }

    /// Advance progress by `dt / duration`. Returns `true` if the transition
    /// just completed (state collapsed to Stable this tick).
    pub fn tick(&mut self, dt: f32, duration: f32) -> bool {
        if let PresetState::Transitioning { to, progress, .. } = self {
            *progress += dt / duration.max(0.05);
            if *progress >= 1.0 {
                *self = PresetState::Stable { current: *to };
                return true;
            }
        }
        false
    }

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

    const S: TransitionStyle = TransitionStyle::Crossfade;
    const D: f32 = 2.0;

    fn weights(s: PresetState) -> Vec<(usize, f32)> {
        match s {
            PresetState::Stable { current } => vec![(current, 1.0)],
            PresetState::Transitioning { from, to, progress, .. } => {
                vec![(from, 1.0 - ease(progress)), (to, ease(progress))]
            }
        }
    }

    fn weight_of(s: PresetState, idx: usize) -> f32 {
        weights(s).into_iter().filter(|(i, _)| *i == idx).map(|(_, w)| w).sum()
    }

    #[test]
    fn stable_doesnt_advance() {
        let mut s = PresetState::stable(0);
        assert!(!s.tick(1.0, D));
        assert!(matches!(s, PresetState::Stable { current: 0 }));
    }

    #[test]
    fn transition_progresses() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1, S, 0.0);
        assert!(matches!(s, PresetState::Transitioning { from: 0, to: 1, .. }));
        let _ = s.tick(0.5, D);
        if let PresetState::Transitioning { progress, .. } = s {
            assert!((progress - 0.25).abs() < 1e-6);
        } else {
            panic!("expected transitioning");
        }
    }

    #[test]
    fn transition_completes() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1, S, 0.0);
        assert!(s.tick(D + 0.1, D));
        assert!(matches!(s, PresetState::Stable { current: 1 }));
    }

    #[test]
    fn easing_is_symmetric_and_flat_at_ends() {
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            assert!((ease(1.0 - x) - (1.0 - ease(x))).abs() < 1e-5);
        }
        assert!(ease(0.01) < 0.001 && ease(0.99) > 0.999);
    }

    #[test]
    fn retarget_keeps_dominant_layer_weight() {
        // Early: `from` dominates.
        let mut s = PresetState::stable(0);
        s.begin_transition(1, S, 0.0);
        s.tick(0.3 * D, D);
        let before = weight_of(s, 0);
        s.begin_transition(2, S, 0.0);
        assert!((weight_of(s, 0) - before).abs() < 1e-5);
        assert!(matches!(s, PresetState::Transitioning { from: 0, to: 2, .. }));

        // Late: `to` dominates and becomes the new `from` at the same weight.
        let mut s = PresetState::stable(0);
        s.begin_transition(1, S, 0.0);
        s.tick(0.8 * D, D);
        let before = weight_of(s, 1);
        s.begin_transition(2, S, 0.0);
        assert!((weight_of(s, 1) - before).abs() < 1e-5);
        assert!(matches!(s, PresetState::Transitioning { from: 1, to: 2, .. }));
    }

    #[test]
    fn going_back_reverses_without_a_jump() {
        let mut s = PresetState::stable(0);
        s.begin_transition(1, S, 0.0);
        s.tick(0.7 * D, D);
        let (w0, w1) = (weight_of(s, 0), weight_of(s, 1));
        s.begin_transition(0, S, 0.0);
        assert!((weight_of(s, 0) - w0).abs() < 1e-5);
        assert!((weight_of(s, 1) - w1).abs() < 1e-5);
        assert_eq!(s.destination(), 0);
    }

    #[test]
    fn no_op_transition_to_same_preset() {
        let mut s = PresetState::stable(0);
        s.begin_transition(0, S, 0.0);
        assert!(matches!(s, PresetState::Stable { current: 0 }));
    }

    #[test]
    fn removal_shifts_indices_and_protects_active() {
        let mut s = PresetState::stable(3);
        assert!(s.remove_index(1));
        assert!(matches!(s, PresetState::Stable { current: 2 }));
        assert!(!s.remove_index(2));
        let mut t = PresetState::stable(0);
        t.begin_transition(4, S, 0.0);
        assert!(t.remove_index(2));
        assert!(matches!(t, PresetState::Transitioning { from: 0, to: 3, .. }));
        assert!(!t.remove_index(0));
    }

    #[test]
    fn styles_parse() {
        assert_eq!(TransitionStyle::parse("dissolve"), Some(TransitionStyle::Dissolve));
        assert_eq!(TransitionStyle::parse("Radial-Wipe"), Some(TransitionStyle::Radial));
        assert_eq!(TransitionStyle::parse("sparkle"), None);
    }
}
