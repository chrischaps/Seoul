//! On-screen HUD, drawn over the finished frame (after tonemapping, so it
//! never blooms): startup wordmark, preset toasts, key-toggle notices, help
//! (H), stats (F1) and preset error reports.
//!
//! Text is glyphon over fonts the OS already ships, so nothing is bundled:
//! on Windows Bahnschrift for UI, Malgun Gothic for Hangul and Consolas for
//! numbers; on macOS Avenir Next, Apple SD Gothic Neo and Menlo. Panels and
//! meters are instanced SDF rounded rects.

use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::Align;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer, Viewport, Weight, fontdb,
};
use tracing::{debug, warn};
use wgpu::util::DeviceExt;

use crate::audio::AudioFeatures;
use crate::preset::preset::PresetSpec;
use crate::render::gpu;

#[cfg(not(target_os = "macos"))]
mod os {
    use std::path::PathBuf;

    pub const SANS: &str = "Bahnschrift";
    pub const HANGUL: &str = "Malgun Gothic";
    pub const MONO: &str = "Consolas";
    pub const FONT_FILES: &[&str] = &["bahnschrift.ttf", "malgun.ttf", "consola.ttf", "segoeui.ttf", "seguisym.ttf"];
    pub const FULLSCREEN_KEY: &str = "F11";

    pub fn font_dir() -> PathBuf {
        PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts")
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::path::PathBuf;

    pub const SANS: &str = "Avenir Next";
    pub const HANGUL: &str = "Apple SD Gothic Neo";
    pub const MONO: &str = "Menlo";
    pub const FONT_FILES: &[&str] =
        &["Avenir Next.ttc", "AppleSDGothicNeo.ttc", "Menlo.ttc", "HelveticaNeue.ttc", "Apple Symbols.ttf"];
    pub const FULLSCREEN_KEY: &str = "⌃⌘F";

    pub fn font_dir() -> PathBuf {
        PathBuf::from("/System/Library/Fonts")
    }
}

use os::{HANGUL, MONO, SANS};
const MAX_RECTS: usize = 256;

const WORDMARK_FADE_IN: f32 = 0.8;
const WORDMARK_HOLD: f32 = 2.2;
const WORDMARK_FADE_OUT: f32 = 1.2;
const TOAST_FADE_IN: f32 = 0.35;
const TOAST_FADE_OUT: f32 = 0.9;
const NOTICE_SECONDS: f32 = 1.6;

/// Everything the HUD shows that comes from outside it.
pub struct HudInput<'a> {
    pub features: &'a AudioFeatures,
    pub preset: &'a PresetSpec,
    pub favorite: bool,
    pub locked: bool,
    pub auto: bool,
    pub transition: Option<f32>,
    pub error: Option<&'a str>,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct RectInstance {
    rect: [f32; 4],
    color: [f32; 4],
    style: [f32; 2],
}

/// A text buffer that only re-shapes when its content changes.
struct Label {
    buffer: Buffer,
    text: String,
    width: f32,
}

impl Label {
    fn new(fs: &mut FontSystem) -> Self {
        Self {
            buffer: Buffer::new(fs, Metrics::new(16.0, 20.0)),
            text: String::new(),
            width: -1.0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn set(
        &mut self,
        fs: &mut FontSystem,
        text: &str,
        family: &str,
        size: f32,
        line: f32,
        weight: Weight,
        spacing: f32,
        width: f32,
        align: Option<Align>,
    ) {
        if self.text == text && self.width == width && self.buffer.metrics().font_size == size {
            return;
        }
        self.text = text.to_owned();
        self.width = width;
        self.buffer.set_metrics(Metrics::new(size, line));
        self.buffer.set_size(Some(width), None);
        let attrs = Attrs::new().family(Family::Name(family)).weight(weight).letter_spacing(spacing);
        self.buffer.set_text(text, &attrs, Shaping::Advanced, align);
        self.buffer.shape_until_scroll(fs, false);
    }

    /// Laid-out (width, height) in pixels.
    fn extent(&self) -> (f32, f32) {
        let mut w: f32 = 0.0;
        let mut h: f32 = 0.0;
        for run in self.buffer.layout_runs() {
            w = w.max(run.line_w);
            h = h.max(run.line_top + run.line_height);
        }
        (w, h)
    }
}

struct Toast {
    started: f32,
    accent: [f32; 3],
}

pub struct Hud {
    font_system: FontSystem,
    swash: SwashCache,
    _cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text: TextRenderer,

    rect_pipeline: wgpu::RenderPipeline,
    rect_buf: wgpu::Buffer,
    screen_buf: wgpu::Buffer,
    screen_bg: wgpu::BindGroup,
    rects: Vec<RectInstance>,
    rect_count: u32,

    size: (u32, u32),
    scale: f32,
    clock: f32,
    fps: f32,

    pub enabled: bool,
    pub show_help: bool,
    pub show_stats: bool,
    pub toast_seconds: f32,
    wordmark_from: Option<f32>,
    toast: Option<Toast>,
    last_preset: String,
    notice: Option<(String, f32)>,
    audio_status: String,

    labels: Labels,
}

impl Hud {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, scale: f64) -> Self {
        let mut font_system = load_fonts();
        let swash = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        let shader = gpu::shader_module(device, "seoul.hud.wgsl", include_str!("../../shaders/hud.wgsl"));
        let screen_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.hud.screen"),
            contents: &[0u8; 16],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.hud.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let screen_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.hud.bg"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buf.as_entire_binding(),
            }],
        });
        let rect_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("seoul.hud.rects"),
            size: (MAX_RECTS * std::mem::size_of::<RectInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.hud.layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("seoul.hud.rects"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_rect"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x2],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_rect"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let labels = Labels::new(&mut font_system);

        Self {
            font_system,
            swash,
            _cache: cache,
            viewport,
            atlas,
            text,
            rect_pipeline,
            rect_buf,
            screen_buf,
            screen_bg,
            rects: Vec::with_capacity(MAX_RECTS),
            rect_count: 0,
            size: (1, 1),
            scale: scale as f32,
            clock: 0.0,
            fps: 60.0,
            enabled: true,
            show_help: false,
            show_stats: false,
            toast_seconds: 3.0,
            wordmark_from: Some(0.0),
            toast: None,
            last_preset: String::new(),
            notice: None,
            audio_status: String::new(),
            labels,
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale as f32;
    }

    pub fn set_audio_status(&mut self, status: &str) {
        if self.audio_status != status {
            self.audio_status = status.to_owned();
        }
    }

    /// Skip the startup wordmark (e.g. for screenshot tours).
    pub fn skip_intro(&mut self) {
        self.wordmark_from = None;
    }

    /// Show a short centered notice, e.g. "★ Favorited".
    pub fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), self.clock));
    }

    /// Lay out everything for this frame. Call before `render`.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), dt: f32, input: &HudInput) {
        self.size = size;
        self.clock += dt;
        if dt > 0.0 {
            self.fps += (1.0 / dt - self.fps) * (1.0 - (-dt / 0.5).exp());
        }
        self.rects.clear();

        // A new destination preset → new toast (skipped behind the wordmark).
        if input.preset.name != self.last_preset {
            self.last_preset = input.preset.name.clone();
            let c = input.preset.palette.colors[1];
            self.toast = Some(Toast {
                started: self.clock,
                accent: [c[0], c[1], c[2]],
            });
        }

        let mut areas: Vec<(LabelId, f32, f32, f32)> = Vec::new();
        if self.enabled {
            self.layout_wordmark(&mut areas);
            self.layout_toast(input, &mut areas);
            self.layout_notice(&mut areas);
            if let Some(err) = input.error {
                self.layout_error(err, &mut areas);
            }
        }
        if self.show_stats {
            self.layout_stats(input, &mut areas);
        }
        if self.show_help {
            self.layout_help(&mut areas);
        }

        self.rect_count = self.rects.len().min(MAX_RECTS) as u32;
        if self.rect_count > 0 {
            queue.write_buffer(&self.rect_buf, 0, bytemuck::cast_slice(&self.rects[..self.rect_count as usize]));
        }
        let screen = [size.0 as f32, size.1 as f32, 0.0, 0.0];
        queue.write_buffer(&self.screen_buf, 0, bytemuck::cast_slice(&screen));
        self.viewport.update(
            queue,
            Resolution {
                width: size.0,
                height: size.1,
            },
        );

        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: size.0 as i32,
            bottom: size.1 as i32,
        };
        let text_areas: Vec<TextArea> = areas
            .iter()
            .filter(|(_, _, _, a)| *a > 0.004)
            .map(|&(id, x, y, alpha)| {
                let (label, rgb) = self.labels.get(id);
                TextArea {
                    buffer: &label.buffer,
                    left: x,
                    top: y,
                    scale: 1.0,
                    bounds,
                    default_color: Color::rgba(rgb[0], rgb[1], rgb[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8),
                    custom_glyphs: &[],
                }
            })
            .collect();
        if let Err(e) = self.text.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas,
            &mut self.swash,
        ) {
            debug!("hud text prepare failed: {e}");
        }
    }

    /// Draw the prepared HUD over `target` (whatever is already there stays).
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut rpass = gpu::color_pass(encoder, "seoul.hud", target, wgpu::LoadOp::Load);
        if self.rect_count > 0 {
            rpass.set_pipeline(&self.rect_pipeline);
            rpass.set_bind_group(0, &self.screen_bg, &[]);
            rpass.set_vertex_buffer(0, self.rect_buf.slice(..));
            rpass.draw(0..6, 0..self.rect_count);
        }
        if let Err(e) = self.text.render(&self.atlas, &self.viewport, &mut rpass) {
            debug!("hud text render failed: {e}");
        }
    }

    /// Release atlas space for glyphs that are no longer shown.
    pub fn trim(&mut self) {
        self.atlas.trim();
    }

    // ---- Layout -------------------------------------------------------------

    fn u(&self) -> f32 {
        self.scale
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4], radius: f32) {
        if self.rects.len() < MAX_RECTS && color[3] > 0.003 {
            self.rects.push(RectInstance {
                rect: [x, y, w, h],
                color,
                style: [radius, 0.75],
            });
        }
    }

    fn layout_wordmark(&mut self, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let Some(from) = self.wordmark_from else {
            return;
        };
        let t = self.clock - from;
        let alpha = envelope(t, WORDMARK_FADE_IN, WORDMARK_HOLD, WORDMARK_FADE_OUT);
        if t > WORDMARK_FADE_IN + WORDMARK_HOLD + WORDMARK_FADE_OUT {
            self.wordmark_from = None;
            // The first preset's toast waited behind the wordmark; start it now.
            if let Some(toast) = &mut self.toast {
                toast.started = self.clock;
            }
            return;
        }
        let u = self.u();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let fs = &mut self.font_system;
        self.labels.wordmark.set(fs, "서울", HANGUL, 84.0 * u, 100.0 * u, Weight::NORMAL, 0.12, w, Some(Align::Center));
        self.labels.wordmark_sub
            .set(fs, "SEOUL", SANS, 13.0 * u, 18.0 * u, Weight::NORMAL, 0.9, w, Some(Align::Center));
        // Rise gently as it fades in.
        let rise = (1.0 - (t / WORDMARK_FADE_IN).clamp(0.0, 1.0)).powi(2) * 10.0 * u;
        let y = h * 0.5 - 70.0 * u + rise;
        areas.push((LabelId::WordmarkShadow, 0.0, y + 2.0 * u, alpha * 0.5));
        areas.push((LabelId::Wordmark, 0.0, y, alpha));
        areas.push((LabelId::WordmarkSub, 0.0, y + 104.0 * u, alpha * 0.8));
    }

    fn layout_toast(&mut self, input: &HudInput, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let Some(toast) = &self.toast else {
            return;
        };
        // Wait for the wordmark to clear before the first toast.
        let start = match self.wordmark_from {
            Some(_) => return,
            None => toast.started,
        };
        let t = self.clock - start;
        if t > TOAST_FADE_IN + self.toast_seconds + TOAST_FADE_OUT {
            return;
        }
        let alpha = envelope(t, TOAST_FADE_IN, self.toast_seconds, TOAST_FADE_OUT);
        let accent = toast.accent;
        let u = self.u();
        let h = self.size.1 as f32;
        let margin = 44.0 * u;
        let max_w = (self.size.0 as f32 - 2.0 * margin).min(620.0 * u);

        let spec = input.preset;
        let sub = match (spec.author.is_empty(), spec.description.is_empty()) {
            (false, false) => format!("{}  ·  {}", spec.description, spec.author),
            (true, false) => spec.description.clone(),
            (false, true) => spec.author.clone(),
            (true, true) => String::new(),
        };
        let mut badges = Vec::new();
        if input.favorite {
            badges.push("★");
        }
        if input.locked {
            badges.push("LOCKED");
        }
        let fs = &mut self.font_system;
        self.labels.title
            .set(fs, &spec.name, SANS, 40.0 * u, 46.0 * u, Weight::NORMAL, 0.02, max_w, None);
        self.labels.subtitle
            .set(fs, &sub, SANS, 15.0 * u, 21.0 * u, Weight::NORMAL, 0.03, max_w, None);
        self.labels.badge
            .set(fs, &badges.join("   "), SANS, 13.0 * u, 18.0 * u, Weight::NORMAL, 0.25, 300.0 * u, None);

        let (tw, th) = self.labels.title.extent();
        let (_, sh) = self.labels.subtitle.extent();
        let block = th + 6.0 * u + sh;
        let top = h - margin - block;
        // Slide in from the left a touch.
        let slide = (1.0 - (t / TOAST_FADE_IN).clamp(0.0, 1.0)).powi(3) * -16.0 * u;
        let x = margin + slide;

        self.rect(x, top - 14.0 * u, 44.0 * u, 3.0 * u, srgb(accent, alpha), 1.5 * u);
        areas.push((LabelId::TitleShadow, x + 1.5 * u, top + 2.0 * u, alpha * 0.55));
        areas.push((LabelId::Title, x, top, alpha));
        areas.push((LabelId::SubtitleShadow, x + 1.0 * u, top + th + 6.0 * u + 1.5 * u, alpha * 0.5));
        areas.push((LabelId::Subtitle, x, top + th + 6.0 * u, alpha * 0.78));
        if !badges.is_empty() {
            areas.push((LabelId::Badge, x + tw + 16.0 * u, top + 18.0 * u, alpha * 0.9));
        }
    }

    fn layout_notice(&mut self, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let Some((text, at)) = &self.notice else {
            return;
        };
        let t = self.clock - at;
        if t > NOTICE_SECONDS + 0.4 {
            self.notice = None;
            return;
        }
        let alpha = envelope(t, 0.12, NOTICE_SECONDS, 0.4);
        let u = self.u();
        let text = text.clone();
        self.labels.notice_label
            .set(&mut self.font_system, &text, SANS, 15.0 * u, 20.0 * u, Weight::NORMAL, 0.06, 600.0 * u, None);
        let (w, h) = self.labels.notice_label.extent();
        let pad = (16.0 * u, 9.0 * u);
        let x = (self.size.0 as f32 - w) * 0.5;
        let y = 34.0 * u;
        self.rect(x - pad.0, y - pad.1, w + 2.0 * pad.0, h + 2.0 * pad.1, [0.0, 0.0, 0.0, 0.55 * alpha], 99.0);
        areas.push((LabelId::Notice, x, y, alpha));
    }

    fn layout_error(&mut self, err: &str, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let u = self.u();
        let width = (self.size.0 as f32 - 80.0 * u).min(720.0 * u);
        // Keep it to something readable; the full text is in the log.
        let body: String = err.lines().take(14).collect::<Vec<_>>().join("\n");
        let fs = &mut self.font_system;
        self.labels.error_title
            .set(fs, "PRESET ERROR", SANS, 13.0 * u, 18.0 * u, Weight::NORMAL, 0.25, width, None);
        self.labels.error_body
            .set(fs, &body, MONO, 12.5 * u, 17.0 * u, Weight::NORMAL, 0.0, width - 36.0 * u, None);
        let (_, bh) = self.labels.error_body.extent();
        let (x, y) = (32.0 * u, 32.0 * u);
        let h = 18.0 * u + 10.0 * u + bh + 32.0 * u;
        self.rect(x, y, width, h, [0.02, 0.0, 0.0, 0.82], 10.0 * u);
        self.rect(x, y, 4.0 * u, h, srgb([0.95, 0.25, 0.22], 1.0), 2.0 * u);
        areas.push((LabelId::ErrorTitle, x + 20.0 * u, y + 16.0 * u, 1.0));
        areas.push((LabelId::ErrorBody, x + 20.0 * u, y + 16.0 * u + 28.0 * u, 0.95));
    }

    fn layout_stats(&mut self, input: &HudInput, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let u = self.u();
        let f = input.features;
        let onoff = |b: bool| if b { "on " } else { "off" };
        let transition = match input.transition {
            Some(p) => format!("transition {:>3.0}%", p * 100.0),
            None => String::from("stable"),
        };
        let text = format!(
            "{:>4.0} fps  {:>5.1} ms\n{:>5.1} bpm  conf {:>3.0}%\nbeat #{:<5}  phase {:.2}\nauto {}  lock {}\n{}\naudio  {}",
            self.fps,
            1000.0 / self.fps.max(1.0),
            f.bpm,
            f.bpm_confidence * 100.0,
            f.beat_count as u32,
            f.beat_phase,
            onoff(input.auto),
            onoff(input.locked),
            transition,
            self.audio_status,
        );
        let panel_w = 300.0 * u;
        let fs = &mut self.font_system;
        self.labels.stats
            .set(fs, &text, MONO, 13.0 * u, 18.0 * u, Weight::NORMAL, 0.0, panel_w - 36.0 * u, None);
        let (_, th) = self.labels.stats.extent();

        let x = self.size.0 as f32 - panel_w - 24.0 * u;
        let y = 24.0 * u;
        let meters_h = 3.0 * 12.0 * u + 52.0 * u;
        let panel_h = 18.0 * u + th + 14.0 * u + meters_h + 18.0 * u;
        self.rect(x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.62], 12.0 * u);
        areas.push((LabelId::Stats, x + 18.0 * u, y + 16.0 * u, 0.92));

        // Band meters: fill = instantaneous, tick = ~1 s average. The beat
        // envelope lights the bass meter.
        let inner_w = panel_w - 36.0 * u;
        let mut my = y + 18.0 * u + th + 14.0 * u;
        let bands = [
            (f.bass, f.bass_att, [0.98, 0.42, 0.40]),
            (f.mid, f.mid_att, [0.98, 0.80, 0.36]),
            (f.treble, f.treble_att, [0.42, 0.78, 0.98]),
        ];
        for (i, (v, att, c)) in bands.into_iter().enumerate() {
            let bx = x + 18.0 * u;
            self.rect(bx, my, inner_w, 6.0 * u, [1.0, 1.0, 1.0, 0.08], 3.0 * u);
            let glow = if i == 0 { 0.65 + 0.35 * f.beat } else { 0.8 };
            self.rect(bx, my, inner_w * v.clamp(0.0, 1.0), 6.0 * u, srgb(c, glow), 3.0 * u);
            self.rect(bx + inner_w * att.clamp(0.0, 1.0) - 1.0 * u, my - 2.0 * u, 2.0 * u, 10.0 * u, [1.0, 1.0, 1.0, 0.7], 1.0 * u);
            my += 12.0 * u;
        }

        // Spectrum strip.
        my += 6.0 * u;
        let strip_h = 40.0 * u;
        let bar_w = inner_w / 64.0;
        for (k, &s) in f.spectrum.iter().enumerate() {
            let t = k as f32 / 63.0;
            let c = [0.40 + 0.55 * t, 0.55 + 0.25 * (1.0 - t), 0.98 - 0.45 * t];
            let bh = (strip_h * s.clamp(0.0, 1.0)).max(1.0 * u);
            self.rect(x + 18.0 * u + k as f32 * bar_w, my + strip_h - bh, (bar_w - 1.0 * u).max(1.0), bh, srgb(c, 0.85), 0.0);
        }
    }

    fn layout_help(&mut self, areas: &mut Vec<(LabelId, f32, f32, f32)>) {
        let u = self.u();
        let keys = format!("Space\nBackspace\nR\nA\nL\nF\n1 – 9\nX\nP\nF1\nH\n{}\nEsc", os::FULLSCREEN_KEY);
        const DESC: &str = "Next preset\nPrevious preset\nShuffle\nAuto-advance\nLock this preset\nFavorite\nJump to a favorite\nHide this preset\nScreenshot\nStats\nThis help\nFullscreen\nQuit";
        let fs = &mut self.font_system;
        self.labels.help_title
            .set(fs, "서울  ·  KEYS", SANS, 13.0 * u, 18.0 * u, Weight::NORMAL, 0.3, 400.0 * u, None);
        self.labels.help_keys
            .set(fs, &keys, MONO, 15.0 * u, 25.0 * u, Weight::NORMAL, 0.0, 140.0 * u, None);
        self.labels.help_desc
            .set(fs, DESC, SANS, 15.0 * u, 25.0 * u, Weight::NORMAL, 0.02, 260.0 * u, None);
        let (_, kh) = self.labels.help_keys.extent();
        let panel_w = 420.0 * u;
        let panel_h = 30.0 * u + 30.0 * u + kh + 26.0 * u;
        let x = (self.size.0 as f32 - panel_w) * 0.5;
        let y = (self.size.1 as f32 - panel_h) * 0.5;
        self.rect(x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.7], 16.0 * u);
        areas.push((LabelId::HelpTitle, x + 32.0 * u, y + 26.0 * u, 0.7));
        areas.push((LabelId::HelpKeys, x + 32.0 * u, y + 60.0 * u, 0.95));
        areas.push((LabelId::HelpDesc, x + 172.0 * u, y + 60.0 * u, 0.85));
    }

}

/// All HUD text buffers, kept apart from the font system so text areas
/// can borrow them while glyphon borrows the rest mutably.
struct Labels {
    wordmark: Label,
    wordmark_sub: Label,
    title: Label,
    badge: Label,
    subtitle: Label,
    notice_label: Label,
    help_keys: Label,
    help_desc: Label,
    help_title: Label,
    stats: Label,
    error_title: Label,
    error_body: Label,
}

impl Labels {
    fn new(fs: &mut FontSystem) -> Self {
        Self {
            wordmark: Label::new(fs),
            wordmark_sub: Label::new(fs),
            title: Label::new(fs),
            badge: Label::new(fs),
            subtitle: Label::new(fs),
            notice_label: Label::new(fs),
            help_keys: Label::new(fs),
            help_desc: Label::new(fs),
            help_title: Label::new(fs),
            stats: Label::new(fs),
            error_title: Label::new(fs),
            error_body: Label::new(fs),
        }
    }

    fn get(&self, id: LabelId) -> (&Label, [u8; 3]) {
        const WHITE: [u8; 3] = [245, 246, 250];
        const BLACK: [u8; 3] = [0, 0, 0];
        match id {
            LabelId::Wordmark => (&self.wordmark, WHITE),
            LabelId::WordmarkShadow => (&self.wordmark, BLACK),
            LabelId::WordmarkSub => (&self.wordmark_sub, WHITE),
            LabelId::Title => (&self.title, WHITE),
            LabelId::TitleShadow => (&self.title, BLACK),
            LabelId::Subtitle => (&self.subtitle, WHITE),
            LabelId::SubtitleShadow => (&self.subtitle, BLACK),
            LabelId::Badge => (&self.badge, [255, 214, 120]),
            LabelId::Notice => (&self.notice_label, WHITE),
            LabelId::HelpTitle => (&self.help_title, WHITE),
            LabelId::HelpKeys => (&self.help_keys, [255, 214, 120]),
            LabelId::HelpDesc => (&self.help_desc, WHITE),
            LabelId::Stats => (&self.stats, WHITE),
            LabelId::ErrorTitle => (&self.error_title, [255, 120, 110]),
            LabelId::ErrorBody => (&self.error_body, WHITE),
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum LabelId {
    Wordmark,
    WordmarkShadow,
    WordmarkSub,
    Title,
    TitleShadow,
    Subtitle,
    SubtitleShadow,
    Badge,
    Notice,
    HelpTitle,
    HelpKeys,
    HelpDesc,
    Stats,
    ErrorTitle,
    ErrorBody,
}

/// Fade in over `a`, hold `hold`, fade out over `b`; eased.
fn envelope(t: f32, a: f32, hold: f32, b: f32) -> f32 {
    let x = if t < a {
        t / a
    } else if t < a + hold {
        1.0
    } else {
        1.0 - (t - a - hold) / b
    };
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// sRGB color + alpha → linear RGBA for the sRGB swapchain.
fn srgb(c: [f32; 3], alpha: f32) -> [f32; 4] {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [lin(c[0]), lin(c[1]), lin(c[2]), alpha]
}

/// Load just the handful of system fonts the HUD uses (fast), or fall back
/// to a full system scan if they're missing.
fn load_fonts() -> FontSystem {
    let dir = os::font_dir();
    let sources: Vec<fontdb::Source> = os::FONT_FILES
        .iter()
        .map(|f| dir.join(f))
        .filter(|p| p.exists())
        .map(fontdb::Source::File)
        .collect();
    if sources.len() < 3 {
        warn!("HUD fonts not found in {}; scanning system fonts", dir.display());
        return FontSystem::new();
    }
    FontSystem::new_with_fonts(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shape() {
        assert_eq!(envelope(0.0, 1.0, 1.0, 1.0), 0.0);
        assert_eq!(envelope(1.5, 1.0, 1.0, 1.0), 1.0);
        assert_eq!(envelope(3.5, 1.0, 1.0, 1.0), 0.0);
        assert!((envelope(0.5, 1.0, 1.0, 1.0) - 0.5).abs() < 1e-6);
    }
}
