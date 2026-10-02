//! `seoul.toml`: user settings, hot-reloaded while running.
//!
//! Every key is optional; anything missing takes the built-in default.
//! Command-line flags override the file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use tracing::{info, warn};

use crate::preset::transition::TransitionStyle;
use crate::render::post::PostParams;

pub const DEFAULT_PATH: &str = "seoul.toml";

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Preset to start on (by name).
    pub start_preset: Option<String>,
    pub fullscreen: bool,
    /// Monitor index for fullscreen (0 = primary as winit enumerates them).
    pub monitor: Option<usize>,
    /// Feedback resolution relative to the window.
    pub render_scale: f32,
    pub auto: AutoConfig,
    pub transition: TransitionConfig,
    pub post: PostDefaults,
    pub hud: HudConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            start_preset: None,
            fullscreen: false,
            monitor: None,
            render_scale: 1.0,
            auto: AutoConfig::default(),
            transition: TransitionConfig::default(),
            post: PostDefaults::default(),
            hud: HudConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoConfig {
    pub enabled: bool,
    /// Earliest a preset may change; then it waits for a downbeat.
    pub min_interval: f32,
    /// Change regardless of beats after this long.
    pub max_interval: f32,
}

impl Default for AutoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_interval: 20.0,
            max_interval: 45.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransitionConfig {
    /// crossfade | dissolve | radial | clock | zoom | random
    pub style: String,
    pub duration: f32,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            style: "random".into(),
            duration: 3.0,
        }
    }
}

impl TransitionConfig {
    /// Styles to cycle through; empty means a random style per transition.
    pub fn styles(&self) -> Result<Vec<TransitionStyle>> {
        parse_styles(&self.style)
    }
}

/// "random", one style, or a comma-separated list to cycle through.
pub fn parse_styles(s: &str) -> Result<Vec<TransitionStyle>> {
    if s.trim().eq_ignore_ascii_case("random") {
        return Ok(Vec::new());
    }
    s.split(',')
        .map(|part| {
            TransitionStyle::parse(part.trim()).ok_or_else(|| {
                anyhow!("transition style '{}' (crossfade|dissolve|radial|clock|zoom|random)", part.trim())
            })
        })
        .collect()
}

/// Global look defaults; presets' `[post]` tables override these.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PostDefaults {
    pub exposure: Option<f32>,
    pub bloom: Option<f32>,
    pub bloom_threshold: Option<f32>,
    pub chroma: Option<f32>,
    pub vignette: Option<f32>,
    pub grain: Option<f32>,
    pub saturation: Option<f32>,
    pub contrast: Option<f32>,
}

impl PostDefaults {
    pub fn resolve(&self) -> PostParams {
        let d = PostParams::default();
        PostParams {
            exposure: self.exposure.unwrap_or(d.exposure),
            bloom: self.bloom.unwrap_or(d.bloom),
            bloom_threshold: self.bloom_threshold.unwrap_or(d.bloom_threshold),
            chroma: self.chroma.unwrap_or(d.chroma),
            vignette: self.vignette.unwrap_or(d.vignette),
            grain: self.grain.unwrap_or(d.grain),
            saturation: self.saturation.unwrap_or(d.saturation),
            contrast: self.contrast.unwrap_or(d.contrast),
            ..d
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HudConfig {
    /// Preset-name toasts and error toasts.
    pub enabled: bool,
    /// Show the F1 stats panel at startup.
    pub stats: bool,
    /// How long a preset toast stays up, in seconds.
    pub toast_seconds: f32,
}

impl Default for HudConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stats: false,
            toast_seconds: 3.0,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.transition.styles()?;
        Ok(cfg)
    }

    /// Load `path`; a missing file means defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("read config {}", path.display())),
        }
    }
}

/// Watches the config file and yields a freshly parsed `Config` when it
/// changes (debounced). Parse errors are logged and the old config kept.
pub struct ConfigWatch {
    path: PathBuf,
    rx: Receiver<()>,
    pending_since: Option<Instant>,
    _watcher: RecommendedWatcher,
}

impl ConfigWatch {
    pub fn new(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let dir = path.parent().ok_or_else(|| anyhow!("config path has no parent"))?.to_path_buf();
        let name = path.file_name().map(|n| n.to_owned());
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                && event.paths.iter().any(|p| p.file_name() == name.as_deref())
            {
                let _ = tx.send(());
            }
        })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(Self {
            path,
            rx,
            pending_since: None,
            _watcher: watcher,
        })
    }

    pub fn poll(&mut self) -> Option<Config> {
        while self.rx.try_recv().is_ok() {
            self.pending_since = Some(Instant::now());
        }
        let since = self.pending_since?;
        if since.elapsed() < Duration::from_millis(150) {
            return None;
        }
        self.pending_since = None;
        match Config::load(&self.path) {
            Ok(cfg) => {
                info!(path = %self.path.display(), "config reloaded");
                Some(cfg)
            }
            Err(e) => {
                warn!("config reload failed, keeping previous: {e:#}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_all_defaults() {
        let c = Config::parse("").unwrap();
        assert!(!c.auto.enabled);
        assert_eq!(c.render_scale, 1.0);
        assert!(c.transition.styles().unwrap().is_empty());
        assert_eq!(c.post.resolve(), PostParams::default());
    }

    #[test]
    fn sections_override_defaults() {
        let c = Config::parse(
            "start_preset = \"Ink\"\n[auto]\nenabled = true\nmin_interval = 10\n[transition]\nstyle = \"dissolve\"\n[post]\nbloom = 0.9\n",
        )
        .unwrap();
        assert_eq!(c.start_preset.as_deref(), Some("Ink"));
        assert!(c.auto.enabled);
        assert_eq!(c.auto.min_interval, 10.0);
        assert_eq!(c.auto.max_interval, 45.0);
        assert_eq!(c.transition.styles().unwrap(), vec![TransitionStyle::Dissolve]);
        assert_eq!(
            parse_styles("dissolve, clock").unwrap(),
            vec![TransitionStyle::Dissolve, TransitionStyle::Clock]
        );
        assert_eq!(c.post.resolve().bloom, 0.9);
    }

    #[test]
    fn rejects_typos_and_bad_styles() {
        assert!(Config::parse("[auto]\nenabeld = true\n").is_err());
        assert!(Config::parse("[transition]\nstyle = \"sparkle\"\n").is_err());
    }

    #[test]
    fn shipped_config_parses() {
        let text = include_str!("../seoul.toml");
        Config::parse(text).unwrap();
    }
}
