//! Library of loaded presets: navigation, curation, auto-advance,
//! transitions, per-frame evaluation, and hot reload.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use arrayvec::ArrayVec;
use notify::RecommendedWatcher;
use rand::RngExt;
use tracing::{info, warn};

use crate::audio::AudioFeatures;
use crate::preset::curation::{Curation, ShuffleBag};
use crate::preset::expr::EvalContext;
use crate::preset::preset::PresetSpec;
use crate::preset::shader::{
    CompositeLayouts, build_composite_pipeline, compile_composite_shader, make_palette_resources,
};
use crate::preset::transition::{PresetState, TransitionStyle, ease};
use crate::preset::watcher::{self, ReloadEvent};
use crate::render::mask::TransitionMask;
use crate::render::particles::{ParticleParams, ParticlePlan};
use crate::render::post::PostParams;
use crate::render::warp::{WarpLayouts, WarpParams, build_warp_pipeline};

/// Editors often write a file in several steps; wait for quiet before reloading.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(120);
/// Below this tempo confidence, any beat counts as a "downbeat".
const DOWNBEAT_CONFIDENCE: f32 = 0.4;

pub struct Preset {
    pub spec: PresetSpec,
    pub pipeline: wgpu::RenderPipeline,
    /// Custom warp, if the preset ships one.
    pub warp_pipeline: Option<wgpu::RenderPipeline>,
    // Kept alive to back `palette_bg`.
    _palette_buf: wgpu::Buffer,
    pub palette_bg: wgpu::BindGroup,
}

/// GPU layouts a preset needs to build its pipelines.
pub struct PresetLayouts {
    pub composite: CompositeLayouts,
    pub warp: WarpLayouts,
}

impl Preset {
    fn build(device: &wgpu::Device, layouts: &PresetLayouts, spec: PresetSpec) -> Result<Self> {
        let body = std::fs::read_to_string(&spec.composite_path)
            .with_context(|| format!("read composite shader {}", spec.composite_path.display()))?;
        let file = spec.composite_path.display().to_string();
        let shader = compile_composite_shader(device, &body, &spec.name, &file)?;
        let pipeline = build_composite_pipeline(device, &layouts.composite, &shader, &spec.name)?;

        let warp_pipeline = match &spec.warp_path {
            None => None,
            Some(path) => {
                let body = std::fs::read_to_string(path)
                    .with_context(|| format!("read warp shader {}", path.display()))?;
                let file = path.display().to_string();
                Some(build_warp_pipeline(device, &layouts.warp, &body, &spec.name, &file)?)
            }
        };

        let (palette_buf, palette_bg) =
            make_palette_resources(device, &layouts.composite, &spec.palette, &spec.name);
        Ok(Self {
            spec,
            pipeline,
            warp_pipeline,
            _palette_buf: palette_buf,
            palette_bg,
        })
    }

    fn uses_file(&self, file_name: &std::ffi::OsStr) -> bool {
        self.spec.composite_path.file_name() == Some(file_name)
            || self.spec.warp_path.as_ref().and_then(|p| p.file_name()) == Some(file_name)
    }
}

/// Runtime behavior knobs, from config (and live-editable).
#[derive(Clone, Debug)]
pub struct LibrarySettings {
    pub post_defaults: PostParams,
    pub auto_advance: bool,
    pub auto_min: f32,
    pub auto_max: f32,
    /// `None` = random style per transition.
    pub transition_style: Option<TransitionStyle>,
    pub transition_duration: f32,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            post_defaults: PostParams::default(),
            auto_advance: false,
            auto_min: 20.0,
            auto_max: 45.0,
            transition_style: None,
            transition_duration: 3.0,
        }
    }
}

pub struct PresetLibrary {
    presets: Vec<Preset>,
    state: PresetState,
    settings: LibrarySettings,
    curation: Curation,
    bag: ShuffleBag,
    locked: bool,
    last_advance_time: f32,
    last_beat_count: f32,
    layouts: PresetLayouts,
    dir: PathBuf,
    reload_rx: Option<Receiver<ReloadEvent>>,
    pending_reloads: HashMap<PathBuf, Instant>,
    last_error: Option<String>,
    // Kept alive for the library's lifetime; dropped → watch ends.
    _watcher: Option<RecommendedWatcher>,
}

pub struct FramePlan<'a> {
    /// One masked warp per active preset (two during a transition).
    pub warps: ArrayVec<WarpStep<'a>, 2>,
    pub post: PostParams,
    pub draws: ArrayVec<CompositeDraw<'a>, 2>,
    /// Particle layer settings + the palette to color them with.
    pub particles: Option<(ParticlePlan, &'a wgpu::BindGroup)>,
    /// Present while a transition is running.
    pub transition: Option<TransitionMask>,
}

pub struct WarpStep<'a> {
    pub params: WarpParams,
    /// `None` = built-in warp.
    pub pipeline: Option<&'a wgpu::RenderPipeline>,
    pub palette_bg: &'a wgpu::BindGroup,
}

pub struct CompositeDraw<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub palette_bg: &'a wgpu::BindGroup,
}

impl PresetLibrary {
    pub fn load(
        device: &wgpu::Device,
        dir: &Path,
        layouts: PresetLayouts,
        settings: LibrarySettings,
        curation: Curation,
    ) -> Result<Self> {
        let specs = scan_directory(dir)?;
        let mut presets = Vec::with_capacity(specs.len());
        for spec in specs {
            let name = spec.name.clone();
            match Preset::build(device, &layouts, spec) {
                Ok(p) => presets.push(p),
                Err(e) => warn!("preset '{}' failed to build: {:#}", name, e),
            }
        }
        if presets.is_empty() {
            return Err(anyhow!(
                "no usable presets in {} — place at least one .toml + .wgsl pair",
                dir.display()
            ));
        }

        // Start on the preset named "default" if one exists; otherwise the first alphabetically.
        let start = presets
            .iter()
            .position(|p| p.spec.name.eq_ignore_ascii_case("default"))
            .unwrap_or(0);

        info!(count = presets.len(), start = presets[start].spec.name, "preset library loaded");
        for p in &presets {
            info!(name = p.spec.name, "  preset");
        }

        // Hot-reload: non-fatal if it can't start (log + continue without it)
        let (watcher, reload_rx) = match watcher::spawn(dir) {
            Ok((w, r)) => (Some(w), Some(r)),
            Err(e) => {
                warn!("hot-reload disabled: {:#}", e);
                (None, None)
            }
        };

        Ok(Self {
            presets,
            state: PresetState::stable(start),
            settings,
            curation,
            bag: ShuffleBag::default(),
            locked: false,
            last_advance_time: 0.0,
            last_beat_count: 0.0,
            layouts,
            dir: dir.to_path_buf(),
            reload_rx,
            pending_reloads: HashMap::new(),
            last_error: None,
            _watcher: watcher,
        })
    }

    pub fn set_settings(&mut self, settings: LibrarySettings) {
        self.settings = settings;
    }

    /// Most recent preset load/compile error, cleared by the next success.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    // ---- Hot reload --------------------------------------------------------

    /// Collect file events and apply the ones that have settled. Called
    /// once per frame from the renderer.
    pub fn poll_reloads(&mut self, device: &wgpu::Device) {
        if let Some(rx) = &self.reload_rx {
            let now = Instant::now();
            while let Ok(ReloadEvent::FileChanged(p)) = rx.try_recv() {
                self.pending_reloads.insert(p, now);
            }
        }
        if self.pending_reloads.is_empty() {
            return;
        }
        let now = Instant::now();
        let ready: Vec<PathBuf> = self
            .pending_reloads
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= RELOAD_DEBOUNCE)
            .map(|(p, _)| p.clone())
            .collect();
        for path in ready {
            self.pending_reloads.remove(&path);
            self.apply_reload(device, &path);
        }
    }

    fn report(&mut self, msg: String) {
        warn!("{msg}");
        self.last_error = Some(msg);
    }

    fn apply_reload(&mut self, device: &wgpu::Device, path: &Path) {
        let Some(file_name) = path.file_name() else {
            return;
        };
        match path.extension().and_then(|s| s.to_str()) {
            Some("toml") => {
                let idx = self
                    .presets
                    .iter()
                    .position(|p| p.spec.source_path.file_name() == Some(file_name));
                if !path.exists() {
                    if let Some(idx) = idx {
                        self.remove_preset(idx);
                    }
                    return;
                }
                self.load_toml(device, path, idx);
            }
            Some("wgsl") => {
                // A deleted shader is handled via its TOML's removal event.
                if !path.exists() {
                    return;
                }
                let indices: Vec<usize> = self
                    .presets
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.uses_file(file_name))
                    .map(|(i, _)| i)
                    .collect();
                if indices.is_empty() {
                    // Maybe a new preset whose TOML loaded before its shader existed.
                    self.load_unloaded(device);
                    return;
                }
                for idx in indices {
                    let toml = self.presets[idx].spec.source_path.clone();
                    if toml.exists() {
                        self.load_toml(device, &toml, Some(idx));
                    }
                }
            }
            _ => {}
        }
    }

    /// (Re)build the preset at `toml`, replacing `existing` or appending.
    /// On failure the previous version — if any — keeps running.
    fn load_toml(&mut self, device: &wgpu::Device, toml: &Path, existing: Option<usize>) {
        let built = PresetSpec::load(toml).and_then(|spec| Preset::build(device, &self.layouts, spec));
        match built {
            Ok(preset) => {
                let name = preset.spec.name.clone();
                match existing {
                    Some(idx) => self.presets[idx] = preset,
                    None => {
                        self.presets.push(preset);
                        self.bag.clear();
                    }
                }
                self.last_error = None;
                info!(preset = name, added = existing.is_none(), "preset reloaded");
            }
            Err(e) => self.report(format!("{}: {e:#}", toml.display())),
        }
    }

    fn load_unloaded(&mut self, device: &wgpu::Device) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut todo = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");
            let known = self
                .presets
                .iter()
                .any(|p| p.spec.source_path.file_name() == path.file_name());
            if is_toml && !known {
                todo.push(path);
            }
        }
        for path in todo {
            self.load_toml(device, &path, None);
        }
    }

    fn remove_preset(&mut self, idx: usize) {
        if self.presets.len() <= 1 || !self.state.remove_index(idx) {
            warn!(preset = self.presets[idx].spec.name, "preset file removed but it is in use; keeping it");
            return;
        }
        let p = self.presets.remove(idx);
        self.bag.clear();
        info!(preset = p.spec.name, "preset removed");
    }

    // ---- Queries -----------------------------------------------------------

    pub fn current(&self) -> &PresetSpec {
        &self.presets[self.state.destination()].spec
    }

    pub fn current_name(&self) -> &str {
        &self.current().name
    }

    pub fn len(&self) -> usize {
        self.presets.len()
    }

    pub fn name_at(&self, idx: usize) -> &str {
        &self.presets[idx].spec.name
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.presets.iter().position(|p| p.spec.name.eq_ignore_ascii_case(name))
    }

    pub fn is_favorite(&self) -> bool {
        self.curation.is_favorite(self.current_name())
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn auto_advance(&self) -> bool {
        self.settings.auto_advance
    }

    /// Transition progress 0..1, if one is running.
    pub fn transition_progress(&self) -> Option<f32> {
        match self.state {
            PresetState::Transitioning { progress, .. } => Some(progress),
            PresetState::Stable { .. } => None,
        }
    }

    fn visible(&self, idx: usize) -> bool {
        !self.curation.is_hidden(&self.presets[idx].spec.name)
    }

    // ---- Navigation --------------------------------------------------------

    fn go(&mut self, target: usize) {
        let style = self.settings.transition_style.unwrap_or_else(TransitionStyle::random);
        let seed = rand::rng().random_range(0.0..1.0);
        self.state.begin_transition(target, style, seed);
    }

    /// Step through the library in order, skipping hidden presets.
    fn step(&mut self, dir: isize) {
        let n = self.presets.len() as isize;
        let mut idx = self.state.destination() as isize;
        for _ in 0..n {
            idx = (idx + dir).rem_euclid(n);
            if self.visible(idx as usize) {
                self.go(idx as usize);
                return;
            }
        }
    }

    pub fn next(&mut self) {
        self.step(1);
    }

    pub fn prev(&mut self) {
        self.step(-1);
    }

    /// Shuffle-bag pick: everything visible plays once (favorites twice)
    /// before anything repeats.
    pub fn random(&mut self) {
        let eligible: Vec<(usize, u32)> = (0..self.presets.len())
            .filter(|&i| self.visible(i))
            .map(|i| (i, if self.curation.is_favorite(&self.presets[i].spec.name) { 2 } else { 1 }))
            .collect();
        if let Some(pick) = self.bag.draw(&eligible, self.state.destination()) {
            self.go(pick);
        }
    }

    /// Hard cut (no transition) to preset `idx`.
    pub fn cut_to(&mut self, idx: usize) {
        self.state = PresetState::stable(idx.min(self.presets.len() - 1));
    }

    /// Jump to the n-th favorite (0-based, library order).
    pub fn jump_favorite(&mut self, n: usize) -> bool {
        let fav = (0..self.presets.len())
            .filter(|&i| self.curation.is_favorite(&self.presets[i].spec.name))
            .nth(n);
        match fav {
            Some(i) => {
                self.go(i);
                true
            }
            None => false,
        }
    }

    // ---- Curation ----------------------------------------------------------

    pub fn toggle_favorite(&mut self) -> bool {
        let name = self.current_name().to_owned();
        self.bag.clear();
        self.curation.toggle_favorite(&name)
    }

    /// Hide (or unhide) the current preset. Hiding moves on immediately.
    pub fn toggle_hidden(&mut self) -> bool {
        let name = self.current_name().to_owned();
        let hidden = self.curation.toggle_hidden(&name);
        self.bag.clear();
        if hidden {
            self.next();
        }
        hidden
    }

    pub fn toggle_locked(&mut self) -> bool {
        self.locked = !self.locked;
        self.locked
    }

    pub fn toggle_auto_advance(&mut self) -> bool {
        self.settings.auto_advance = !self.settings.auto_advance;
        self.settings.auto_advance
    }

    // ---- Per frame ---------------------------------------------------------

    /// Advance transition state + evaluate auto-advance.
    pub fn tick(&mut self, features: &AudioFeatures, dt: f32) {
        self.state.tick(dt, self.settings.transition_duration);

        let beat_now = features.beat_count != self.last_beat_count;
        self.last_beat_count = features.beat_count;

        let s = &self.settings;
        if !s.auto_advance || self.locked || !matches!(self.state, PresetState::Stable { .. }) {
            return;
        }
        let since = features.time - self.last_advance_time;
        // Prefer to change on a bar line when the tempo is trusted; with
        // no beat at all (silence), give up at max_interval.
        let on_downbeat = beat_now
            && (features.bpm_confidence < DOWNBEAT_CONFIDENCE || (features.beat_count as u32).is_multiple_of(4));
        if since >= s.auto_max || (since >= s.auto_min && on_downbeat) {
            self.last_advance_time = features.time;
            self.random();
            info!(preset = self.current_name(), "auto-advance");
        }
    }

    /// Produce the render work for this frame.
    pub fn frame_plan(&self, features: &AudioFeatures) -> FramePlan<'_> {
        let ctx = EvalContext::new(features);
        let (active, transition): (ArrayVec<(usize, f32), 2>, _) = match self.state {
            PresetState::Stable { current } => ([(current, 1.0)].into_iter().collect(), None),
            PresetState::Transitioning {
                from,
                to,
                progress,
                style,
                seed,
            } => {
                let p = ease(progress);
                (
                    [(from, 1.0 - p), (to, p)].into_iter().collect(),
                    Some(TransitionMask { progress: p, style, seed }),
                )
            }
        };

        let mut plan = FramePlan {
            warps: ArrayVec::new(),
            post: self.settings.post_defaults,
            draws: ArrayVec::new(),
            particles: None,
            transition,
        };
        let mut post: Option<PostParams> = None;
        let mut particles: Option<(ParticleParams, f32, &wgpu::BindGroup)> = None;
        for (i, &(idx, weight)) in active.iter().enumerate() {
            let p = &self.presets[idx];
            let mut params = p.spec.mapping.eval(&ctx, p.spec.edge);
            // Zoom style: the outgoing preset's feedback accelerates away.
            if i == 0 && matches!(transition, Some(TransitionMask { style: TransitionStyle::Zoom, .. })) {
                params.zoom *= 1.0 + 0.05 * (1.0 - weight);
            }
            plan.warps.push(WarpStep {
                params,
                pipeline: p.warp_pipeline.as_ref(),
                palette_bg: &p.palette_bg,
            });
            plan.draws.push(CompositeDraw {
                pipeline: &p.pipeline,
                palette_bg: &p.palette_bg,
            });

            // Fold post and particles with running weights so a single
            // active preset passes through unchanged.
            let this_post = p.spec.post.apply(&self.settings.post_defaults, &ctx);
            post = Some(match post {
                None => this_post,
                Some(prev) => PostParams::lerp(&prev, &this_post, weight),
            });
            if let Some(pp) = p.spec.particles {
                particles = Some(match particles {
                    None => (pp, weight, &p.palette_bg),
                    Some((prev, w, bg)) => (
                        ParticleParams::lerp(&prev, &pp, weight),
                        w + weight,
                        if weight >= 0.5 { &p.palette_bg } else { bg },
                    ),
                });
            }
        }
        if let Some(post) = post {
            plan.post = post;
        }
        plan.particles = particles.map(|(params, weight, bg)| (ParticlePlan { params, weight }, bg));
        plan
    }
}

fn scan_directory(dir: &Path) -> Result<Vec<PresetSpec>> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("open preset dir {}", dir.display()))?;
    let mut specs = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match PresetSpec::load(&path) {
            Ok(spec) => specs.push(spec),
            Err(e) => warn!("failed to load preset {}: {:#}", path.display(), e),
        }
    }
    specs.sort_by_key(|s| s.name.to_ascii_lowercase());
    Ok(specs)
}
