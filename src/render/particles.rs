//! GPU particle layer: a compute pass advects up to `MAX_PARTICLES` through a
//! curl-noise flow field with audio-coupled forces, then instanced sprites
//! are added into the feedback texture (before the composite) so the warp
//! smears them into trails.
//!
//! One particle population persists across presets; a transition just
//! crossfades the parameters and the draw weight.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::render::feedback::FEEDBACK_FORMAT;

pub const MAX_PARTICLES: u32 = 131_072;
const WORKGROUP: u32 = 256;
const PARTICLE_BYTES: u64 = 32;

const SIM_SOURCE: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    "\n",
    include_str!("../../shaders/particles_common.wgsl"),
    "\n",
    include_str!("../../shaders/particles_sim.wgsl"),
);
const DRAW_SOURCE: &str = concat!(
    include_str!("../../shaders/common.wgsl"),
    "\n",
    include_str!("../../shaders/particles_common.wgsl"),
    "\n",
    include_str!("../../shaders/particles_draw.wgsl"),
);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SpawnShape {
    #[default]
    Center,
    Ring,
    Edges,
    Waveform,
    Random,
}

impl SpawnShape {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "center" => Self::Center,
            "ring" => Self::Ring,
            "edges" => Self::Edges,
            "waveform" => Self::Waveform,
            "random" => Self::Random,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// One palette entry (0–3).
    Palette(u32),
    /// Palette ramp over each particle's lifetime.
    Ramp,
    /// Palette ramp by spawn tag, brightened by that spectrum band.
    Spectrum,
}

impl Default for ColorMode {
    fn default() -> Self {
        Self::Palette(3)
    }
}

/// Per-preset particle settings (TOML `[particles]`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParticleParams {
    pub count: u32,
    pub spawn: SpawnShape,
    /// Initial speed, screen heights per second.
    pub speed: f32,
    /// Curl-noise acceleration strength.
    pub flow: f32,
    /// Curl-noise spatial frequency.
    pub flow_scale: f32,
    /// Velocity damping per second.
    pub drag: f32,
    /// Sprite radius in pixels at 1080p.
    pub size: f32,
    /// Mean lifetime in seconds.
    pub life: f32,
    pub color: ColorMode,
    /// Outward kick on each detected beat.
    pub burst: f32,
    /// Outward acceleration × bass.
    pub bass_push: f32,
    pub gravity: f32,
    pub intensity: f32,
}

impl Default for ParticleParams {
    fn default() -> Self {
        Self {
            count: 20_000,
            spawn: SpawnShape::default(),
            speed: 0.15,
            flow: 0.5,
            flow_scale: 2.0,
            drag: 0.8,
            size: 3.0,
            life: 3.0,
            color: ColorMode::default(),
            burst: 0.0,
            bass_push: 0.0,
            gravity: 0.0,
            intensity: 0.05,
        }
    }
}

impl ParticleParams {
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        let late = t >= 0.5;
        Self {
            count: a.count.max(b.count),
            spawn: if late { b.spawn } else { a.spawn },
            speed: l(a.speed, b.speed),
            flow: l(a.flow, b.flow),
            flow_scale: l(a.flow_scale, b.flow_scale),
            drag: l(a.drag, b.drag),
            size: l(a.size, b.size),
            life: l(a.life, b.life),
            color: if late { b.color } else { a.color },
            burst: l(a.burst, b.burst),
            bass_push: l(a.bass_push, b.bass_push),
            gravity: l(a.gravity, b.gravity),
            intensity: l(a.intensity, b.intensity),
        }
    }
}

/// What the particle layer should do this frame.
#[derive(Copy, Clone, Debug)]
pub struct ParticlePlan {
    pub params: ParticleParams,
    /// Draw weight (transition crossfade); 0 skips the layer entirely.
    pub weight: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ParticleUniforms {
    dt: f32,
    time: f32,
    count: u32,
    spawn: u32,
    speed: f32,
    flow: f32,
    flow_scale: f32,
    drag: f32,
    size: f32,
    life: f32,
    color_mode: u32,
    color_index: u32,
    burst: f32,
    bass_push: f32,
    gravity: f32,
    intensity: f32,
    beat_trigger: f32,
    aspect: f32,
    resolution: [f32; 2],
    frame: u32,
    _pad: [u32; 3],
}

const _: () = assert!(std::mem::size_of::<ParticleUniforms>() == 96);

pub struct ParticleSystem {
    sim_pipeline: wgpu::ComputePipeline,
    draw_pipeline: wgpu::RenderPipeline,
    _particles: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    sim_bg: wgpu::BindGroup,
    draw_bg: wgpu::BindGroup,
    last_beat_count: f32,
}

/// Frame inputs the particle layer needs besides its params.
pub struct ParticleFrame<'a> {
    pub dt: f32,
    pub time: f32,
    pub frame: u32,
    pub aspect: f32,
    pub resolution: (u32, u32),
    pub beat_count: f32,
    pub audio_bg: &'a wgpu::BindGroup,
    pub palette_bg: &'a wgpu::BindGroup,
}

impl ParticleSystem {
    pub fn new(
        device: &wgpu::Device,
        audio_layout: &wgpu::BindGroupLayout,
        palette_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self> {
        let particles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("seoul.particles"),
            size: MAX_PARTICLES as u64 * PARTICLE_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            // Zeroed: age 0 ≥ life 0, so everything spawns on the first frame.
            mapped_at_creation: false,
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.particles.uniforms"),
            contents: bytemuck::bytes_of(&ParticleUniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let layout_for = |read_only: bool, stages: wgpu::ShaderStages| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("seoul.particles.bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: stages,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: stages,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            })
        };
        let sim_layout = layout_for(false, wgpu::ShaderStages::COMPUTE);
        let draw_layout = layout_for(true, wgpu::ShaderStages::VERTEX_FRAGMENT);
        let bind = |layout: &wgpu::BindGroupLayout| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("seoul.particles.bg"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: particles.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: uniforms.as_entire_binding(),
                    },
                ],
            })
        };
        let sim_bg = bind(&sim_layout);
        let draw_bg = bind(&draw_layout);

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sim_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("seoul.particles.sim"),
            source: wgpu::ShaderSource::Wgsl(SIM_SOURCE.into()),
        });
        let draw_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("seoul.particles.draw"),
            source: wgpu::ShaderSource::Wgsl(DRAW_SOURCE.into()),
        });

        let sim_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.particles.sim.layout"),
            bind_group_layouts: &[Some(&sim_layout), Some(audio_layout), Some(palette_layout)],
            immediate_size: 0,
        });
        let sim_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("seoul.particles.sim"),
            layout: Some(&sim_pl),
            module: &sim_module,
            entry_point: Some("cs_update"),
            compilation_options: Default::default(),
            cache: None,
        });

        let draw_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.particles.draw.layout"),
            bind_group_layouts: &[Some(&draw_layout), Some(audio_layout), Some(palette_layout)],
            immediate_size: 0,
        });
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Constant,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let draw_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("seoul.particles.draw"),
            layout: Some(&draw_pl),
            vertex: wgpu::VertexState {
                module: &draw_module,
                entry_point: Some("vs_particle"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_module,
                entry_point: Some("fs_particle"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FEEDBACK_FORMAT,
                    blend: Some(additive),
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
            anyhow::bail!("particle shaders failed to build: {e}");
        }

        Ok(Self {
            sim_pipeline,
            draw_pipeline,
            _particles: particles,
            uniforms,
            sim_bg,
            draw_bg,
            last_beat_count: 0.0,
        })
    }

    /// Simulate, then add sprites into `target` (the feedback write texture).
    pub fn run(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        plan: &ParticlePlan,
        f: &ParticleFrame,
    ) {
        let beat_trigger = f.beat_count != self.last_beat_count;
        self.last_beat_count = f.beat_count;
        if plan.weight <= 0.0 {
            return;
        }

        let pp = &plan.params;
        let count = pp.count.min(MAX_PARTICLES);
        let (color_mode, color_index) = match pp.color {
            ColorMode::Palette(i) => (0, i.min(3)),
            ColorMode::Ramp => (1, 0),
            ColorMode::Spectrum => (2, 0),
        };
        let u = ParticleUniforms {
            dt: f.dt,
            time: f.time,
            count,
            spawn: pp.spawn as u32,
            speed: pp.speed,
            flow: pp.flow,
            flow_scale: pp.flow_scale,
            drag: pp.drag,
            size: pp.size,
            life: pp.life.max(0.05),
            color_mode,
            color_index,
            burst: pp.burst,
            bass_push: pp.bass_push,
            gravity: pp.gravity,
            intensity: pp.intensity,
            beat_trigger: if beat_trigger { 1.0 } else { 0.0 },
            aspect: f.aspect,
            resolution: [f.resolution.0 as f32, f.resolution.1 as f32],
            frame: f.frame,
            _pad: [0; 3],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("seoul.particles.sim"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.sim_pipeline);
            cpass.set_bind_group(0, &self.sim_bg, &[]);
            cpass.set_bind_group(1, f.audio_bg, &[]);
            cpass.set_bind_group(2, f.palette_bg, &[]);
            cpass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
        }

        // Energy per frame scales with frame time, like the composite.
        let w = (plan.weight * (f.dt * 60.0).clamp(0.0, 6.0)) as f64;
        let mut rpass = crate::render::gpu::color_pass(encoder, "seoul.particles.draw", target, wgpu::LoadOp::Load);
        rpass.set_blend_constant(wgpu::Color { r: w, g: w, b: w, a: w });
        rpass.set_pipeline(&self.draw_pipeline);
        rpass.set_bind_group(0, &self.draw_bg, &[]);
        rpass.set_bind_group(1, f.audio_bg, &[]);
        rpass.set_bind_group(2, f.palette_bg, &[]);
        rpass.draw(0..6, 0..count);
    }
}
