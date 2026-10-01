//! Library of loaded presets + navigation + per-frame evaluation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

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
use crate::preset::transition::{PresetState, lerp};
use crate::preset::watcher::{self, ReloadEvent};
use crate::render::mesh::WarpParams;

const AUTO_ADVANCE_INTERVAL: f32 = 25.0;
const AUTO_ADVANCE_BEAT_THRESHOLD: f32 = 0.7;
const DECAY_MIN: f32 = 0.5;
const DECAY_MAX: f32 = 0.9999;

pub struct Preset {
    pub spec: PresetSpec,
    #[allow(dead_code)]
    pub shader: wgpu::ShaderModule,
    pub pipeline: wgpu::RenderPipeline,
    #[allow(dead_code)]
    pub palette_buf: wgpu::Buffer,
    pub palette_bg: wgpu::BindGroup,
}

impl Preset {
    fn build(
        device: &wgpu::Device,
        layouts: &CompositeLayouts,
        spec: PresetSpec,
    ) -> Result<Self> {
        let body = std::fs::read_to_string(&spec.composite_path)
            .with_context(|| format!("read composite shader {}", spec.composite_path.display()))?;
        let shader = compile_composite_shader(device, &body, &spec.name)?;
        let pipeline = build_composite_pipeline(device, layouts, &shader, &spec.name);
        let (palette_buf, palette_bg) =
            make_palette_resources(device, layouts, &spec.palette, &spec.name);
        Ok(Self {
            spec,
            shader,
            pipeline,
            palette_buf,
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
    #[allow(dead_code)]
    dir: PathBuf,
    reload_rx: Option<Receiver<ReloadEvent>>,
    // Kept alive for the library's lifetime; dropped → watch ends.
    #[allow(dead_code)]
    watcher: Option<RecommendedWatcher>,
}

pub struct FramePlan<'a> {
    pub warp: WarpParams,
    pub decay: f32,
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
    ) -> Result<Self> {
        let specs = scan_directory(dir)?;
        if specs.is_empty() {
            return Err(anyhow!(
                "no presets found in {} — place at least one .toml + .wgsl pair",
                dir.display()
            ));
        }

        let mut presets = Vec::with_capacity(specs.len());
        for spec in specs {
            let name = spec.name.clone();
            match Preset::build(device, &layouts, spec) {
                Ok(p) => presets.push(p),
                Err(e) => warn!("preset '{}' failed to build: {}", name, e),
            }
        }

        if presets.is_empty() {
            return Err(anyhow!("every preset failed to build — see warnings above"));
        }

        // Start on the preset named "default" if one exists; otherwise the first alphabetically.
        let start = presets
            .iter()
            .position(|p| p.spec.name.eq_ignore_ascii_case("default"))
            .unwrap_or(0);

        info!(
            count = presets.len(),
            start = presets[start].spec.name,
            "preset library loaded"
        );
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
            reload_rx,
            watcher,
        })
    }

    /// Drain reload events and apply them. Called each frame from the renderer.
    pub fn poll_reloads(&mut self, device: &wgpu::Device) {
        let Some(rx) = &self.reload_rx else {
            return;
        };

        // Drain and dedupe by path — editors often fire multiple events per save.
        let mut changed: HashSet<PathBuf> = HashSet::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ReloadEvent::FileChanged(p) => {
                    changed.insert(p);
                }
            }
        }
        if changed.is_empty() {
            return;
        }
        for path in changed {
            self.apply_reload(device, &path);
        }
    }

    fn apply_reload(&mut self, device: &wgpu::Device, path: &Path) {
        let Some(file_name) = path.file_name() else {
            return;
        };
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

        if ext == "toml" {
            // Find which preset owns this toml by leaf filename.
            let idx = self
                .presets
                .iter()
                .position(|p| p.spec.source_path.file_name() == Some(file_name));
            let Some(idx) = idx else {
                return;
            };

            match PresetSpec::load(path) {
                Ok(spec) => {
                    let name = spec.name.clone();
                    match Preset::build(device, &self.layouts, spec) {
                        Ok(new_preset) => {
                            self.presets[idx] = new_preset;
                            info!(preset = name, "reloaded (toml)");
                        }
                        Err(e) => warn!("preset '{}' reload failed: {:#}", name, e),
                    }
                }
                Err(e) => warn!("preset TOML {} reload failed: {:#}", path.display(), e),
            }
        } else if ext == "wgsl" {
            // Potentially multiple presets share the same shader — apply to all.
            let indices: Vec<usize> = self
                .presets
                .iter()
                .enumerate()
                .filter(|(_, p)| p.spec.composite_path.file_name() == Some(file_name))
                .map(|(i, _)| i)
                .collect();
            if indices.is_empty() {
                return;
            }

            let body = match std::fs::read_to_string(path) {
                Ok(b) => b,
                Err(e) => {
                    warn!("read shader {}: {}", path.display(), e);
                    return;
                }
            };

            for idx in indices {
                let name = self.presets[idx].spec.name.clone();
                match compile_composite_shader(device, &body, &name) {
                    Ok(module) => {
                        let pipeline =
                            build_composite_pipeline(device, &self.layouts, &module, &name);
                        self.presets[idx].shader = module;
                        self.presets[idx].pipeline = pipeline;
                        info!(preset = name, "reloaded (wgsl)");
                    }
                    Err(e) => warn!("preset '{}' shader reload failed: {:#}", name, e),
                }
            }
        }
    }

    pub fn current_name(&self) -> &str {
        let idx = match self.state {
            PresetState::Stable { current } => current,
            PresetState::Transitioning { to, .. } => to,
        };
        &self.presets[idx].spec.name
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

    /// Produce the render work for this frame.
    pub fn frame_plan(&self, features: &AudioFeatures) -> FramePlan<'_> {
        let ctx = EvalContext::new(features);
        match self.state {
            PresetState::Stable { current } => {
                let p = &self.presets[current];
                let warp = WarpParams {
                    zoom: p.spec.mapping.zoom.eval(&ctx),
                    rotation: p.spec.mapping.rotation.eval(&ctx),
                    warp_amount: p.spec.mapping.warp_amount.eval(&ctx),
                };
                let decay = p.spec.mapping.decay.eval(&ctx).clamp(DECAY_MIN, DECAY_MAX);
                let mut draws = ArrayVec::new();
                draws.push(CompositeDraw {
                    pipeline: &p.pipeline,
                    palette_bg: &p.palette_bg,
                    intensity: 1.0,
                });
                FramePlan { warp, decay, draws }
            }
            PresetState::Transitioning { from, to, progress } => {
                let a = &self.presets[from];
                let b = &self.presets[to];
                let warp = WarpParams {
                    zoom: lerp(
                        a.spec.mapping.zoom.eval(&ctx),
                        b.spec.mapping.zoom.eval(&ctx),
                        progress,
                    ),
                    rotation: lerp(
                        a.spec.mapping.rotation.eval(&ctx),
                        b.spec.mapping.rotation.eval(&ctx),
                        progress,
                    ),
                    warp_amount: lerp(
                        a.spec.mapping.warp_amount.eval(&ctx),
                        b.spec.mapping.warp_amount.eval(&ctx),
                        progress,
                    ),
                };
                let decay = lerp(
                    a.spec.mapping.decay.eval(&ctx),
                    b.spec.mapping.decay.eval(&ctx),
                    progress,
                )
                .clamp(DECAY_MIN, DECAY_MAX);
                let mut draws = ArrayVec::new();
                draws.push(CompositeDraw {
                    pipeline: &a.pipeline,
                    palette_bg: &a.palette_bg,
                    intensity: 1.0 - progress,
                });
                draws.push(CompositeDraw {
                    pipeline: &b.pipeline,
                    palette_bg: &b.palette_bg,
                    intensity: progress,
                });
                FramePlan { warp, decay, draws }
            }
        }
    }

    pub fn layouts(&self) -> &CompositeLayouts {
        &self.layouts
    }
}

fn scan_directory(dir: &Path) -> Result<Vec<PresetSpec>> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("open preset dir {}", dir.display()))?;
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
    specs.sort_by(|a, b| a.spec_sort_key().cmp(&b.spec_sort_key()));
    Ok(specs)
}

impl PresetSpec {
    fn spec_sort_key(&self) -> String {
        self.name.to_ascii_lowercase()
    }
}
