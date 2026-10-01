//! Pass 1: resample the previous frame through the warp transform.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::render::feedback::FEEDBACK_FORMAT;
use crate::render::gpu;

const DECAY_MIN: f32 = 0.5;
const DECAY_MAX: f32 = 0.9999;
/// Presets are authored as "per frame at 60 Hz".
const REFERENCE_HZ: f32 = 60.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum EdgeMode {
    /// Sample outside [0,1] by reflecting — no smeared border streaks.
    #[default]
    Mirror,
    /// Outside [0,1] fades to black.
    Fade,
    /// Classic clamp-to-edge.
    Clamp,
}

impl EdgeMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "mirror" => Self::Mirror,
            "fade" => Self::Fade,
            "clamp" => Self::Clamp,
            _ => return None,
        })
    }

    fn as_f32(self) -> f32 {
        match self {
            Self::Mirror => 0.0,
            Self::Fade => 1.0,
            Self::Clamp => 2.0,
        }
    }
}

/// Per-frame warp parameters evaluated from the active preset, in the
/// preset author's units: everything is "per frame at 60 Hz" except
/// `hue_shift`, which is radians per second.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WarpParams {
    pub zoom: f32,
    pub rotation: f32,
    pub warp_amount: f32,
    pub decay: f32,
    pub cx: f32,
    pub cy: f32,
    pub dx: f32,
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
    pub warp_scale: f32,
    pub warp_speed: f32,
    pub hue_shift: f32,
    pub blur: f32,
    pub sharpen: f32,
    pub edge: EdgeMode,
}

impl Default for WarpParams {
    /// The identity warp: a straight copy.
    fn default() -> Self {
        Self {
            zoom: 1.0,
            rotation: 0.0,
            warp_amount: 0.0,
            decay: 1.0,
            cx: 0.5,
            cy: 0.5,
            dx: 0.0,
            dy: 0.0,
            sx: 1.0,
            sy: 1.0,
            warp_scale: 1.0,
            warp_speed: 1.0,
            hue_shift: 0.0,
            blur: 0.0,
            sharpen: 0.0,
            edge: EdgeMode::default(),
        }
    }
}

impl WarpParams {
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Self {
            zoom: l(a.zoom, b.zoom),
            rotation: l(a.rotation, b.rotation),
            warp_amount: l(a.warp_amount, b.warp_amount),
            decay: l(a.decay, b.decay),
            cx: l(a.cx, b.cx),
            cy: l(a.cy, b.cy),
            dx: l(a.dx, b.dx),
            dy: l(a.dy, b.dy),
            sx: l(a.sx, b.sx),
            sy: l(a.sy, b.sy),
            warp_scale: l(a.warp_scale, b.warp_scale),
            warp_speed: l(a.warp_speed, b.warp_speed),
            hue_shift: l(a.hue_shift, b.hue_shift),
            blur: l(a.blur, b.blur),
            sharpen: l(a.sharpen, b.sharpen),
            edge: if t < 0.5 { a.edge } else { b.edge },
        }
    }

    /// Convert to GPU uniforms for a frame that took `dt` seconds.
    ///
    /// Multiplicative per-frame quantities (decay, zoom, stretch) are raised
    /// to `k = dt·60`; additive ones (rotation, translation, wobble) scale by
    /// `k`. A 144 Hz display then produces the same motion and trail length
    /// per second as the 60 Hz the preset was written for.
    pub fn to_uniforms(self, dt: f32, time: f32, aspect: f32, size: (u32, u32)) -> WarpUniforms {
        let k = (dt * REFERENCE_HZ).clamp(0.0, 6.0);
        let pos = |x: f32| x.max(0.01);
        WarpUniforms {
            decay: self.decay.clamp(DECAY_MIN, DECAY_MAX).powf(k),
            zoom: pos(self.zoom).powf(k),
            rotation: self.rotation * k,
            warp_amount: self.warp_amount * k,
            cx: self.cx,
            cy: self.cy,
            dx: self.dx * k,
            dy: self.dy * k,
            sx: pos(self.sx).powf(k),
            sy: pos(self.sy).powf(k),
            warp_scale: self.warp_scale,
            warp_speed: self.warp_speed,
            time,
            aspect,
            hue_shift: self.hue_shift * dt,
            blur: 1.0 - (1.0 - self.blur.clamp(0.0, 1.0)).powf(k),
            sharpen: self.sharpen * k,
            edge_mode: self.edge.as_f32(),
            texel: [1.0 / size.0.max(1) as f32, 1.0 / size.1.max(1) as f32],
        }
    }
}

/// GPU layout of `WarpUniforms` in `shaders/warp.wgsl` (80 bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct WarpUniforms {
    pub decay: f32,
    pub zoom: f32,
    pub rotation: f32,
    pub warp_amount: f32,
    pub cx: f32,
    pub cy: f32,
    pub dx: f32,
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
    pub warp_scale: f32,
    pub warp_speed: f32,
    pub time: f32,
    pub aspect: f32,
    pub hue_shift: f32,
    pub blur: f32,
    pub sharpen: f32,
    pub edge_mode: f32,
    pub texel: [f32; 2],
}

const _: () = assert!(std::mem::size_of::<WarpUniforms>() == 80);

pub struct WarpPass {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    uniform_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture_bgs: [wgpu::BindGroup; 2],
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    _identity_buf: wgpu::Buffer,
    identity_bg: wgpu::BindGroup,
}

impl WarpPass {
    pub fn new(device: &wgpu::Device, feedback_views: &[wgpu::TextureView; 2]) -> Self {
        let shader = gpu::shader_module(device, "seoul.warp.wgsl", include_str!("../../shaders/warp.wgsl"));
        let sampler = gpu::linear_clamp_sampler(device, "seoul.warp.sampler");

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.tex.bgl"),
            entries: &[gpu::texture_entry(0), gpu::sampler_entry(1)],
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.uniform.bgl"),
            entries: &[gpu::uniform_entry(0)],
        });

        let make_uniform = |label: &str, u: WarpUniforms| {
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            (buf, bg)
        };
        let identity = WarpParams {
            edge: EdgeMode::Clamp,
            ..Default::default()
        };
        // dt = 1/60 → k = 1, i.e. exactly the identity transform.
        let (uniform_buf, uniform_bg) =
            make_uniform("seoul.warp.uniforms", identity.to_uniforms(1.0 / 60.0, 0.0, 1.0, (1, 1)));
        let (identity_buf, identity_bg) =
            make_uniform("seoul.warp.identity", identity.to_uniforms(1.0 / 60.0, 0.0, 1.0, (1, 1)));

        let pipeline = gpu::fullscreen_pipeline(
            device,
            gpu::FullscreenPipeline {
                label: "seoul.warp.pipeline",
                module: &shader,
                vs: "vs_warp",
                fs: "fs_warp",
                layouts: &[&texture_layout, &uniform_layout],
                format: FEEDBACK_FORMAT,
                blend: None,
            },
        );

        let texture_bgs = Self::make_texture_bgs(device, &texture_layout, &sampler, feedback_views);

        Self {
            pipeline,
            texture_layout,
            uniform_layout,
            sampler,
            texture_bgs,
            uniform_buf,
            uniform_bg,
            _identity_buf: identity_buf,
            identity_bg,
        }
    }

    fn make_texture_bgs(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        views: &[wgpu::TextureView; 2],
    ) -> [wgpu::BindGroup; 2] {
        let make = |view: &wgpu::TextureView| Self::texture_bg(device, layout, sampler, view);
        [make(&views[0]), make(&views[1])]
    }

    fn texture_bg(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.warp.tex.bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Layouts for building custom warp pipelines (group 0: prev + sampler,
    /// group 1: warp uniforms).
    pub fn layouts(&self) -> [&wgpu::BindGroupLayout; 2] {
        [&self.texture_layout, &self.uniform_layout]
    }

    /// Rebuild bind groups after the feedback textures were recreated.
    pub fn rebind(&mut self, device: &wgpu::Device, views: &[wgpu::TextureView; 2]) {
        self.texture_bgs = Self::make_texture_bgs(device, &self.texture_layout, &self.sampler, views);
    }

    pub fn update(&self, queue: &wgpu::Queue, u: &WarpUniforms) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }

    /// Warp the `read_idx` feedback texture into `target`. `custom` replaces
    /// the built-in warp with a preset's own pipeline (same layouts).
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        read_idx: usize,
        custom: Option<&wgpu::RenderPipeline>,
    ) {
        let mut rpass = gpu::color_pass(encoder, "seoul.warp.pass", target, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
        rpass.set_pipeline(custom.unwrap_or(&self.pipeline));
        rpass.set_bind_group(0, &self.texture_bgs[read_idx], &[]);
        rpass.set_bind_group(1, &self.uniform_bg, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Scaled straight copy of `src` into `dst` — used to carry the image
    /// across a feedback resize instead of flashing to black.
    pub fn resample(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bg = Self::texture_bg(device, &self.texture_layout, &self.sampler, src);
        let mut rpass = gpu::color_pass(encoder, "seoul.warp.resample", dst, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &bg, &[]);
        rpass.set_bind_group(1, &self.identity_bg, &[]);
        rpass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> WarpParams {
        WarpParams {
            zoom: 1.02,
            rotation: 0.01,
            decay: 0.96,
            dx: 0.001,
            hue_shift: 0.5,
            ..Default::default()
        }
    }

    #[test]
    fn sixty_hz_is_authoring_identity() {
        let u = params().to_uniforms(1.0 / 60.0, 0.0, 1.0, (64, 64));
        assert!((u.decay - 0.96).abs() < 1e-6);
        assert!((u.zoom - 1.02).abs() < 1e-6);
        assert!((u.rotation - 0.01).abs() < 1e-6);
    }

    #[test]
    fn one_second_matches_across_refresh_rates() {
        // Accumulate one second of decay/zoom/rotation/translation at 60 and
        // 144 Hz: totals must agree.
        let total = |hz: f32| {
            let n = hz as usize;
            let u = params().to_uniforms(1.0 / hz, 0.0, 1.0, (64, 64));
            (
                u.decay.powi(n as i32),
                u.zoom.powi(n as i32),
                u.rotation * n as f32,
                u.dx * n as f32,
                u.hue_shift * n as f32,
            )
        };
        let a = total(60.0);
        let b = total(144.0);
        assert!((a.0 - b.0).abs() < 1e-3, "decay {a:?} vs {b:?}");
        assert!((a.1 - b.1).abs() < 1e-3, "zoom");
        assert!((a.2 - b.2).abs() < 1e-4, "rotation");
        assert!((a.3 - b.3).abs() < 1e-5, "translation");
        assert!((a.4 - b.4).abs() < 1e-4, "hue");
    }

    #[test]
    fn decay_is_clamped_before_scaling() {
        let p = WarpParams {
            decay: 1.5,
            ..Default::default()
        };
        assert!(p.to_uniforms(1.0 / 60.0, 0.0, 1.0, (1, 1)).decay <= DECAY_MAX);
    }
}
