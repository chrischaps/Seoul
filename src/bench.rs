//! `--bench`: how fast each preset draws on this machine. It's a tour that
//! times frames instead of taking screenshots, with vsync off, so the
//! numbers show headroom rather than the display's refresh rate.

use std::fmt::Write as _;
use std::time::Instant;

/// The first second on each preset (the cut, first-use allocations) isn't
/// counted.
pub const WARMUP: f32 = 1.0;

/// Frame rates are judged over windows this long. Single frame times mislead
/// with vsync off: the GPU queue fills and drains, so frames arrive in
/// fast/slow pairs. A window still catches a sustained dip, like a burst of
/// fireworks.
const WINDOW_MS: f32 = 1000.0;

#[derive(Default)]
pub struct Bench {
    last: Option<Instant>,
    frame_ms: Vec<f32>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub preset: String,
    pub fps: f32,
    /// The frame rate of the slowest one-second window.
    pub low_fps: f32,
    pub worst_ms: f32,
}

impl Row {
    pub fn holds_60(&self) -> bool {
        self.low_fps >= 60.0
    }
}

impl Bench {
    /// Once per presented frame; frames during warm-up are timed but not kept.
    pub fn frame(&mut self, counting: bool) {
        let now = Instant::now();
        if let Some(last) = self.last
            && counting
        {
            self.frame_ms.push((now - last).as_secs_f32() * 1000.0);
        }
        self.last = Some(now);
    }

    /// Close out a preset's dwell.
    pub fn finish(&mut self, preset: &str) -> Option<&Row> {
        let row = summarize(preset, &std::mem::take(&mut self.frame_ms))?;
        self.rows.push(row);
        self.rows.last()
    }

    /// The results as CSV, slowest first, under a comment naming the setup.
    pub fn csv(&self, setup: &str) -> String {
        let mut rows = self.rows.clone();
        rows.sort_by(|a, b| a.fps.total_cmp(&b.fps));
        let mut out = format!("# {setup}\npreset,fps,low_fps,worst_ms,holds_60\n");
        for r in &rows {
            let _ = writeln!(out, "{},{:.1},{:.1},{:.2},{}", r.preset, r.fps, r.low_fps, r.worst_ms, r.holds_60());
        }
        out
    }
}

pub fn summarize(preset: &str, frame_ms: &[f32]) -> Option<Row> {
    if frame_ms.is_empty() {
        return None;
    }
    let fps = |ms: &[f32]| 1000.0 * ms.len() as f32 / ms.iter().sum::<f32>();
    // Whole windows only; a run shorter than one window is its own.
    let mut low_fps = f32::INFINITY;
    let mut start = 0;
    let mut elapsed = 0.0;
    for (i, ms) in frame_ms.iter().enumerate() {
        elapsed += ms;
        if elapsed >= WINDOW_MS {
            low_fps = low_fps.min(fps(&frame_ms[start..=i]));
            start = i + 1;
            elapsed = 0.0;
        }
    }
    if low_fps.is_infinite() {
        low_fps = fps(frame_ms);
    }
    Some(Row {
        preset: preset.to_owned(),
        fps: fps(frame_ms),
        low_fps,
        worst_ms: frame_ms.iter().copied().fold(0.0, f32::max),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_slow_pairs_still_hold_60() {
        // What vsync-off pipelining looks like: 2 ms, 18 ms, … = 100 fps.
        let ms: Vec<f32> = (0..400).map(|i| if i % 2 == 0 { 2.0 } else { 18.0 }).collect();
        let r = summarize("Ink", &ms).unwrap();
        assert!((r.fps - 100.0).abs() < 1e-3);
        assert!((r.low_fps - 100.0).abs() < 1e-3);
        assert_eq!(r.worst_ms, 18.0);
        assert!(r.holds_60());
    }

    #[test]
    fn a_sustained_dip_fails_60_even_when_the_mean_passes() {
        // Three seconds at 100 fps, then one at 40: the mean is 85.
        let mut ms = vec![10.0; 300];
        ms.extend([25.0; 40]);
        let r = summarize("Han River", &ms).unwrap();
        assert!(r.fps > 80.0);
        assert!((r.low_fps - 40.0).abs() < 1e-3);
        assert!(!r.holds_60());
    }

    #[test]
    fn csv_lists_slowest_first() {
        let mut b = Bench::default();
        b.rows.push(summarize("Fast", &[5.0]).unwrap());
        b.rows.push(summarize("Slow", &[40.0]).unwrap());
        let csv = b.csv("720x1280");
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "# 720x1280");
        assert!(lines[2].starts_with("Slow,25.0,"));
        assert!(lines[3].starts_with("Fast,200.0,"));
    }

    #[test]
    fn nothing_timed_is_no_row() {
        assert!(Bench::default().finish("Ink").is_none());
    }
}
