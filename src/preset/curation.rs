//! Favorites and hidden presets, persisted by name to `seoul-state.toml`,
//! plus the shuffle bag that picks what plays next.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rand::RngExt;
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const STATE_PATH: &str = "seoul-state.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CurationFile {
    favorites: BTreeSet<String>,
    hidden: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub struct Curation {
    path: Option<PathBuf>,
    data: CurationFile,
}

impl Curation {
    /// Load from `path` (missing or unreadable → empty, with a warning for
    /// the latter). Changes are written back to the same path.
    pub fn load(path: &Path) -> Self {
        let data = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                warn!("ignoring unreadable {}: {e}", path.display());
                CurationFile::default()
            }),
            Err(_) => CurationFile::default(),
        };
        Self {
            path: Some(path.to_path_buf()),
            data,
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let text = toml::to_string_pretty(&self.data).unwrap_or_default();
        let header = "# Written by Seoul: F toggles a favorite, X hides a preset.\n";
        if let Err(e) = std::fs::write(path, format!("{header}{text}")) {
            warn!("could not save {}: {e}", path.display());
        }
    }

    pub fn is_favorite(&self, name: &str) -> bool {
        self.data.favorites.contains(name)
    }

    pub fn is_hidden(&self, name: &str) -> bool {
        self.data.hidden.contains(name)
    }

    /// Returns the new state.
    pub fn toggle_favorite(&mut self, name: &str) -> bool {
        let on = toggle(&mut self.data.favorites, name);
        self.save();
        on
    }

    /// Returns the new state.
    pub fn toggle_hidden(&mut self, name: &str) -> bool {
        let on = toggle(&mut self.data.hidden, name);
        self.save();
        on
    }
}

fn toggle(set: &mut BTreeSet<String>, name: &str) -> bool {
    if set.remove(name) {
        false
    } else {
        set.insert(name.to_owned());
        true
    }
}

/// Draws without replacement until empty, then refills — every eligible
/// preset plays once per cycle (favorites twice) before any repeats.
#[derive(Debug, Default)]
pub struct ShuffleBag {
    bag: Vec<usize>,
}

impl ShuffleBag {
    /// Pick the next index. `eligible` lists (index, weight) pairs;
    /// `current` is avoided unless it's the only option.
    pub fn draw(&mut self, eligible: &[(usize, u32)], current: usize) -> Option<usize> {
        // Drop entries that are no longer eligible (hidden, removed).
        self.bag.retain(|i| eligible.iter().any(|(e, _)| e == i));
        if self.bag.iter().all(|&i| i == current) {
            // New cycle; what's playing now already had its turn.
            self.bag = eligible
                .iter()
                .filter(|&&(i, _)| i != current)
                .flat_map(|&(i, w)| std::iter::repeat_n(i, w as usize))
                .collect();
        }
        let candidates: Vec<usize> = (0..self.bag.len()).filter(|&k| self.bag[k] != current).collect();
        if candidates.is_empty() {
            return eligible.first().map(|&(i, _)| i).filter(|&i| i != current);
        }
        let k = candidates[rand::rng().random_range(0..candidates.len())];
        Some(self.bag.swap_remove(k))
    }

    /// Forget everything (e.g. after the library changes).
    pub fn clear(&mut self) {
        self.bag.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bag_plays_everything_before_repeating() {
        let mut bag = ShuffleBag::default();
        let eligible: Vec<(usize, u32)> = (0..5).map(|i| (i, 1)).collect();
        let mut current = 0;
        let mut seen = BTreeSet::new();
        for _ in 0..4 {
            current = bag.draw(&eligible, current).unwrap();
            seen.insert(current);
        }
        // Four draws from {1,2,3,4} (0 was current at the start) are distinct.
        assert_eq!(seen.len(), 4);
        assert!(!seen.contains(&0));
    }

    #[test]
    fn bag_never_repeats_current_and_honors_weights() {
        let mut bag = ShuffleBag::default();
        let eligible = vec![(0, 1), (1, 2), (2, 1)];
        let mut current = 0;
        let mut counts = [0; 3];
        for _ in 0..4000 {
            let next = bag.draw(&eligible, current).unwrap();
            assert_ne!(next, current);
            counts[next] += 1;
            current = next;
        }
        // The double-weighted favorite plays clearly more often.
        assert!(counts[1] > counts[0] && counts[1] > counts[2], "{counts:?}");
    }

    #[test]
    fn single_eligible_is_returned_only_if_not_current() {
        let mut bag = ShuffleBag::default();
        assert_eq!(bag.draw(&[(3, 1)], 3), None);
        assert_eq!(bag.draw(&[(3, 1)], 0), Some(3));
    }

    #[test]
    fn toggles_flip_membership() {
        let mut c = Curation::default();
        assert!(c.toggle_favorite("Ink"));
        assert!(c.is_favorite("Ink"));
        assert!(!c.toggle_favorite("Ink"));
        assert!(!c.is_favorite("Ink"));
        assert!(c.toggle_hidden("Plasma"));
        assert!(c.is_hidden("Plasma"));
    }
}
