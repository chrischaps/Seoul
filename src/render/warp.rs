//! Pass 1: resample the previous frame through the warp transform.

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::preset::shader::compile_wgsl;
use crate::render::feedback::FEEDBACK_FORMAT;
use crate::render::gpu;
use crate::render::mask::{MASKED_BLEND, MaskPass, MaskSlot};

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

/// common.wgsl + warp_prelude.wgsl: prepended to every warp shader.
pub const WARP_PRELUDE: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    "\n",
    include_str!("../../shaders/warp_prelude.wgsl"),
);
const DEFAULT_WARP: &str = include_str!("../../shaders/warp_default.wgsl");

/// Bind group layouts shared by the built-in warp and every custom one:
/// 0 = prev frame + sampler, 1 = warp uniforms, 2 = audio, 3 = palette.
#[derive(Clone)]
pub struct WarpLayouts {
    texture: wgpu::BindGroupLayout,
    uniform: wgpu::BindGroupLayout,
    palette: wgpu::BindGroupLayout,
    pipeline: wgpu::PipelineLayout,
}

impl WarpLayouts {
    pub fn new(device: &wgpu::Device, audio: &wgpu::BindGroupLayout, palette: &wgpu::BindGroupLayout) -> Self {
        let texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.tex.bgl"),
            entries: &[gpu::texture_entry(0), gpu::sampler_entry(1)],
        });
        let uniform = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.uniform.bgl"),
            entries: &[gpu::uniform_entry(0)],
        });
        let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.warp.pipeline_layout"),
            bind_group_layouts: &[Some(&texture), Some(&uniform), Some(audio), Some(palette)],
            immediate_size: 0,
        });
        Self {
            texture,
            uniform,
            palette: palette.clone(),
            pipeline,
        }
    }
}

/// Compile a warp shader body (built-in or a preset's) into a pipeline.
/// Validation failures come back as `Err`, never a device panic.
pub fn build_warp_pipeline(
    device: &wgpu::Device,
    layouts: &WarpLayouts,
    body: &str,
    label: &str,
    file: &str,
) -> Result<wgpu::RenderPipeline> {
    let module = compile_wgsl(device, WARP_PRELUDE, body, &format!("seoul.warp.shader.{label}"), file)?;
    // Each warp draw adds `mask × warped image` into a cleared target, so a
    // transition can crossfade (or dissolve, wipe…) two presets' feedback
    // dynamics.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!("seoul.warp.pipeline.{label}")),
        layout: Some(&layouts.pipeline),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_warp"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_warp"),
            targets: &[Some(wgpu::ColorTargetState {
                format: FEEDBACK_FORMAT,
                blend: Some(MASKED_BLEND),
                write_mask: wgpu::ColorWrites::COLOR,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(anyhow!("warp pipeline for '{label}' failed: {e}"));
    }
    Ok(pipeline)
}

/// One masked warp draw for this frame.
pub struct WarpDraw<'a> {
    /// `None` = built-in warp.
    pub pipeline: Option<&'a wgpu::RenderPipeline>,
    pub uniforms: WarpUniforms,
    pub palette_bg: &'a wgpu::BindGroup,
}

struct UniformSlot {
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

pub struct WarpPass {
    layouts: WarpLayouts,
    default_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    texture_bgs: [wgpu::BindGroup; 2],
    /// One uniform buffer per simultaneous warp draw (two during transitions).
    slots: [UniformSlot; 2],
    identity: UniformSlot,
    _neutral_palette: wgpu::Buffer,
    neutral_palette_bg: wgpu::BindGroup,
}

impl WarpPass {
    pub fn new(device: &wgpu::Device, feedback_views: &[wgpu::TextureView; 2], layouts: WarpLayouts) -> Result<Self> {
        let sampler = gpu::linear_clamp_sampler(device, "seoul.warp.sampler");
        let default_pipeline = build_warp_pipeline(device, &layouts, DEFAULT_WARP, "builtin", "shaders/warp_default.wgsl")?;

        let slot = |label: &str, u: WarpUniforms| {
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layouts.uniform,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            UniformSlot { buf, bg }
        };
        let identity_params = WarpParams {
            edge: EdgeMode::Clamp,
            ..Default::default()
        };
        // dt = 1/60 → k = 1, i.e. exactly the identity transform.
        let identity_u = identity_params.to_uniforms(1.0 / 60.0, 0.0, 1.0, (1, 1));
        let slots = [slot("seoul.warp.uniforms.0", identity_u), slot("seoul.warp.uniforms.1", identity_u)];
        let identity = slot("seoul.warp.identity", identity_u);

        // The resample path has no preset, but the layout still wants a palette.
        let neutral_palette = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.warp.neutral_palette"),
            contents: &[0u8; 64],
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let neutral_palette_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.warp.neutral_palette"),
            layout: &layouts.palette,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: neutral_palette.as_entire_binding(),
            }],
        });

        let texture_bgs = Self::make_texture_bgs(device, &layouts.texture, &sampler, feedback_views);

        Ok(Self {
            layouts,
            default_pipeline,
            sampler,
            texture_bgs,
            slots,
            identity,
            _neutral_palette: neutral_palette,
            neutral_palette_bg,
        })
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

    /// Rebuild bind groups after the feedback textures were recreated.
    pub fn rebind(&mut self, device: &wgpu::Device, views: &[wgpu::TextureView; 2]) {
        self.texture_bgs = Self::make_texture_bgs(device, &self.layouts.texture, &self.sampler, views);
    }

    /// Warp the `read_idx` feedback texture into `target`: one masked draw
    /// per active preset, summed into a cleared target.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        read_idx: usize,
        audio_bg: &wgpu::BindGroup,
        mask: &MaskPass,
        draws: &[WarpDraw],
    ) {
        for (slot, draw) in self.slots.iter().zip(draws) {
            queue.write_buffer(&slot.buf, 0, bytemuck::bytes_of(&draw.uniforms));
        }
        let mut rpass = gpu::color_pass(encoder, "seoul.warp.pass", target, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
        for (i, (slot, draw)) in self.slots.iter().zip(draws).enumerate() {
            mask.draw(&mut rpass, MaskSlot::warp(i));
            rpass.set_pipeline(draw.pipeline.unwrap_or(&self.default_pipeline));
            rpass.set_bind_group(0, &self.texture_bgs[read_idx], &[]);
            rpass.set_bind_group(1, &slot.bg, &[]);
            rpass.set_bind_group(2, audio_bg, &[]);
            rpass.set_bind_group(3, draw.palette_bg, &[]);
            rpass.draw(0..3, 0..1);
        }
    }

    /// Scaled straight copy of `src` into `dst` — used to carry the image
    /// across a feedback resize instead of flashing to black.
    pub fn resample(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        audio_bg: &wgpu::BindGroup,
    ) {
        let bg = Self::texture_bg(device, &self.layouts.texture, &self.sampler, src);
        // Alpha 1 = full mask weight for the single straight copy.
        let clear = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let mut rpass = gpu::color_pass(encoder, "seoul.warp.resample", dst, wgpu::LoadOp::Clear(clear));
        rpass.set_pipeline(&self.default_pipeline);
        rpass.set_bind_group(0, &bg, &[]);
        rpass.set_bind_group(1, &self.identity.bg, &[]);
        rpass.set_bind_group(2, audio_bg, &[]);
        rpass.set_bind_group(3, &self.neutral_palette_bg, &[]);
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
