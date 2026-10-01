//! Library of loaded presets + navigation + per-frame evaluation + hot reload.

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
use crate::preset::expr::EvalContext;
use crate::preset::preset::PresetSpec;
use crate::preset::shader::{
    CompositeLayouts, build_composite_pipeline, compile_composite_shader, make_palette_resources,
};
use crate::preset::transition::PresetState;
use crate::preset::watcher::{self, ReloadEvent};
use crate::render::post::PostParams;
use crate::render::warp::WarpParams;

const AUTO_ADVANCE_INTERVAL: f32 = 25.0;
const AUTO_ADVANCE_BEAT_THRESHOLD: f32 = 0.7;
/// Editors often write a file in several steps; wait for quiet before reloading.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(120);

pub struct Preset {
    pub spec: PresetSpec,
    pub pipeline: wgpu::RenderPipeline,
    // Kept alive to back `palette_bg`.
    _palette_buf: wgpu::Buffer,
    pub palette_bg: wgpu::BindGroup,
}

impl Preset {
    fn build(device: &wgpu::Device, layouts: &CompositeLayouts, spec: PresetSpec) -> Result<Self> {
        let body = std::fs::read_to_string(&spec.composite_path)
            .with_context(|| format!("read composite shader {}", spec.composite_path.display()))?;
        let file = spec.composite_path.display().to_string();
        let shader = compile_composite_shader(device, &body, &spec.name, &file)?;
        let pipeline = build_composite_pipeline(device, layouts, &shader, &spec.name)?;
        let (palette_buf, palette_bg) = make_palette_resources(device, layouts, &spec.palette, &spec.name);
        Ok(Self {
            spec,
            pipeline,
            _palette_buf: palette_buf,
            palette_bg,
        })
    }
}

pub struct PresetLibrary {
    presets: Vec<Preset>,
    state: PresetState,
    auto_advance: bool,
    last_advance_time: f32,
    layouts: CompositeLayouts,
    dir: PathBuf,
    post_defaults: PostParams,
    reload_rx: Option<Receiver<ReloadEvent>>,
    pending_reloads: HashMap<PathBuf, Instant>,
    last_error: Option<String>,
    // Kept alive for the library's lifetime; dropped → watch ends.
    _watcher: Option<RecommendedWatcher>,
}

pub struct FramePlan<'a> {
    pub warp: WarpParams,
    pub post: PostParams,
    pub draws: ArrayVec<CompositeDraw<'a>, 2>,
}

pub struct CompositeDraw<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub palette_bg: &'a wgpu::BindGroup,
    pub intensity: f32,
}

impl PresetLibrary {
    pub fn load(
        device: &wgpu::Device,
        dir: &Path,
        layouts: CompositeLayouts,
        post_defaults: PostParams,
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
            auto_advance: false,
            last_advance_time: 0.0,
            layouts,
            dir: dir.to_path_buf(),
            post_defaults,
            reload_rx,
            pending_reloads: HashMap::new(),
            last_error: None,
            _watcher: watcher,
        })
    }

    /// Most recent preset load/compile error, cleared by the next success.
    #[allow(dead_code)] // surfaced by the HUD
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

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
                    .filter(|(_, p)| p.spec.composite_path.file_name() == Some(file_name))
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
                    None => self.presets.push(preset),
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
        info!(preset = p.spec.name, "preset removed");
    }

    pub fn current(&self) -> &PresetSpec {
        &self.presets[self.state.destination()].spec
    }

    pub fn current_name(&self) -> &str {
        &self.current().name
    }

    pub fn len(&self) -> usize {
        self.presets.len()
    }

    pub fn next(&mut self) {
        let cur = self.state.destination();
        let nxt = (cur + 1) % self.presets.len();
        self.state.begin_transition(nxt);
    }

    pub fn prev(&mut self) {
        let cur = self.state.destination();
        let prv = (cur + self.presets.len() - 1) % self.presets.len();
        self.state.begin_transition(prv);
    }

    pub fn random(&mut self) {
        if self.presets.len() <= 1 {
            return;
        }
        let cur = self.state.destination();
        let mut rng = rand::rng();
        let mut pick = cur;
        while pick == cur {
            pick = rng.random_range(0..self.presets.len());
        }
        self.state.begin_transition(pick);
    }

    /// Hard cut (no transition) to preset `idx`.
    pub fn cut_to(&mut self, idx: usize) {
        self.state = PresetState::stable(idx.min(self.presets.len() - 1));
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.presets.iter().position(|p| p.spec.name.eq_ignore_ascii_case(name))
    }

    pub fn toggle_auto_advance(&mut self) -> bool {
        self.auto_advance = !self.auto_advance;
        self.auto_advance
    }

    /// Advance transition state + evaluate auto-advance.
    pub fn tick(&mut self, features: &AudioFeatures, dt: f32) {
        self.state.tick(dt);

        if self.auto_advance && matches!(self.state, PresetState::Stable { .. }) {
            let elapsed = features.time - self.last_advance_time;
            if elapsed > AUTO_ADVANCE_INTERVAL && features.beat > AUTO_ADVANCE_BEAT_THRESHOLD {
                self.next();
                self.last_advance_time = features.time;
                info!(preset = self.current_name(), "auto-advance");
            }
        }
    }

    fn eval(&self, idx: usize, ctx: &EvalContext) -> (WarpParams, PostParams) {
        let spec = &self.presets[idx].spec;
        (spec.mapping.eval(ctx, spec.edge), spec.post.apply(&self.post_defaults))
    }

    /// Produce the render work for this frame.
    pub fn frame_plan(&self, features: &AudioFeatures) -> FramePlan<'_> {
        let ctx = EvalContext::new(features);
        let mut draws = ArrayVec::new();
        let draw = |idx: usize, intensity: f32| CompositeDraw {
            pipeline: &self.presets[idx].pipeline,
            palette_bg: &self.presets[idx].palette_bg,
            intensity,
        };
        match self.state {
            PresetState::Stable { current } => {
                let (warp, post) = self.eval(current, &ctx);
                draws.push(draw(current, 1.0));
                FramePlan { warp, post, draws }
            }
            PresetState::Transitioning { from, to, progress } => {
                let (wa, pa) = self.eval(from, &ctx);
                let (wb, pb) = self.eval(to, &ctx);
                draws.push(draw(from, 1.0 - progress));
                draws.push(draw(to, progress));
                FramePlan {
                    warp: WarpParams::lerp(&wa, &wb, progress),
                    post: PostParams::lerp(&pa, &pb, progress),
                    draws,
                }
            }
        }
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
