//! Display passes after the feedback loop: bloom (mip chain) and the final
//! tonemap/grade into the swapchain.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::render::gpu;

const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BLOOM_MAX_LEVELS: u32 = 6;

/// Look-development knobs for the display passes. Global defaults come from
/// config; presets override any subset in their `[post]` table.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PostParams {
    pub exposure: f32,
    pub bloom: f32,
    pub bloom_threshold: f32,
    pub chroma: f32,
    pub vignette: f32,
    pub grain: f32,
    pub led_mask: f32,
    pub led_pitch: f32,
    pub saturation: f32,
    pub contrast: f32,
}

impl Default for PostParams {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            bloom: 0.35,
            bloom_threshold: 0.7,
            chroma: 0.0,
            vignette: 0.3,
            grain: 0.15,
            led_mask: 0.0,
            led_pitch: 9.0,
            saturation: 1.05,
            contrast: 1.0,
        }
    }
}

impl PostParams {
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Self {
            exposure: l(a.exposure, b.exposure),
            bloom: l(a.bloom, b.bloom),
            bloom_threshold: l(a.bloom_threshold, b.bloom_threshold),
            chroma: l(a.chroma, b.chroma),
            vignette: l(a.vignette, b.vignette),
            grain: l(a.grain, b.grain),
            // A dot grid can't half-exist; switch at the midpoint.
            led_mask: if t < 0.5 { a.led_mask } else { b.led_mask },
            led_pitch: if t < 0.5 { a.led_pitch } else { b.led_pitch },
            saturation: l(a.saturation, b.saturation),
            contrast: l(a.contrast, b.contrast),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct BloomUniforms {
    texel: [f32; 2],
    threshold: f32,
    knee: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PostUniforms {
    exposure: f32,
    bloom: f32,
    chroma: f32,
    vignette: f32,
    grain: f32,
    led_mask: f32,
    led_pitch: f32,
    saturation: f32,
    time: f32,
    beat: f32,
    resolution: [f32; 2],
    contrast: f32,
    _pad: [f32; 3],
}

const _: () = assert!(std::mem::size_of::<PostUniforms>() == 64);

struct BloomStep {
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
}

struct BloomChain {
    _texture: wgpu::Texture,
    mip_views: Vec<wgpu::TextureView>,
    /// Prefilter from feedback[i] → mip 0, one per ping-pong texture.
    prefilter: [BloomStep; 2],
    /// down[i]: mip i → mip i+1.
    down: Vec<BloomStep>,
    /// up[i]: mip i+1 → mip i (additive).
    up: Vec<BloomStep>,
}

pub struct PostPass {
    bloom_prefilter: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    bloom_layout: wgpu::BindGroupLayout,
    final_pipeline: wgpu::RenderPipeline,
    final_tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    chain: BloomChain,
    final_tex_bgs: [wgpu::BindGroup; 2],
    final_uniforms: wgpu::Buffer,
    final_uniform_bg: wgpu::BindGroup,
}

impl PostPass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        feedback_views: &[wgpu::TextureView; 2],
        feedback_size: (u32, u32),
    ) -> Self {
        let sampler = gpu::linear_clamp_sampler(device, "seoul.post.sampler");

        let bloom_shader = gpu::shader_module(device, "seoul.bloom.wgsl", include_str!("../../shaders/bloom.wgsl"));
        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.bloom.bgl"),
            entries: &[gpu::texture_entry(0), gpu::sampler_entry(1), gpu::uniform_entry(2)],
        });
        let bloom_pipeline = |fs: &str, blend: Option<wgpu::BlendState>| {
            gpu::fullscreen_pipeline(
                device,
                gpu::FullscreenPipeline {
                    label: &format!("seoul.bloom.{fs}"),
                    module: &bloom_shader,
                    vs: "vs_bloom",
                    fs,
                    layouts: &[&bloom_layout],
                    format: BLOOM_FORMAT,
                    blend,
                },
            )
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let bloom_prefilter = bloom_pipeline("fs_prefilter", None);
        let bloom_down = bloom_pipeline("fs_down", None);
        let bloom_up = bloom_pipeline("fs_up", Some(additive));

        let post_shader = gpu::shader_module(device, "seoul.post.wgsl", include_str!("../../shaders/post.wgsl"));
        let final_tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.post.tex.bgl"),
            entries: &[gpu::texture_entry(0), gpu::texture_entry(1), gpu::sampler_entry(2)],
        });
        let final_uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.post.uniform.bgl"),
            entries: &[gpu::uniform_entry(0)],
        });
        let final_pipeline = gpu::fullscreen_pipeline(
            device,
            gpu::FullscreenPipeline {
                label: "seoul.post.pipeline",
                module: &post_shader,
                vs: "vs_post",
                fs: "fs_post",
                layouts: &[&final_tex_layout, &final_uniform_layout],
                format: surface_format,
                blend: None,
            },
        );
        let final_uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.post.uniforms"),
            contents: bytemuck::bytes_of(&PostUniforms::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let final_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.post.uniform.bg"),
            layout: &final_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: final_uniforms.as_entire_binding(),
            }],
        });

        let chain = build_chain(device, &bloom_layout, &sampler, feedback_views, feedback_size);
        let final_tex_bgs = build_final_bgs(device, &final_tex_layout, &sampler, feedback_views, &chain);

        Self {
            bloom_prefilter,
            bloom_down,
            bloom_up,
            bloom_layout,
            final_pipeline,
            final_tex_layout,
            sampler,
            chain,
            final_tex_bgs,
            final_uniforms,
            final_uniform_bg,
        }
    }

    /// Rebuild the bloom chain and bind groups for new feedback textures.
    pub fn rebind(&mut self, device: &wgpu::Device, views: &[wgpu::TextureView; 2], size: (u32, u32)) {
        self.chain = build_chain(device, &self.bloom_layout, &self.sampler, views, size);
        self.final_tex_bgs = build_final_bgs(device, &self.final_tex_layout, &self.sampler, views, &self.chain);
    }

    pub fn update(
        &self,
        queue: &wgpu::Queue,
        params: &PostParams,
        time: f32,
        beat: f32,
        output_size: (u32, u32),
    ) {
        let threshold = params.bloom_threshold.max(0.0);
        let bu = BloomUniforms {
            texel: [0.0; 2],
            threshold,
            knee: (threshold * 0.5).max(0.05),
        };
        // Only the threshold changes per frame; texel sizes stay as built.
        for step in &self.chain.prefilter {
            queue.write_buffer(&step.uniforms, 8, bytemuck::cast_slice(&[bu.threshold, bu.knee]));
        }

        let levels = self.chain.mip_views.len().max(1) as f32;
        let u = PostUniforms {
            exposure: params.exposure,
            // The up-chain sums every level into mip 0; normalize.
            bloom: params.bloom / levels,
            chroma: params.chroma,
            vignette: params.vignette,
            grain: params.grain,
            led_mask: params.led_mask,
            led_pitch: params.led_pitch,
            saturation: params.saturation,
            time,
            beat,
            resolution: [output_size.0 as f32, output_size.1 as f32],
            contrast: params.contrast,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.final_uniforms, 0, bytemuck::bytes_of(&u));
    }

    /// Run bloom on feedback texture `src_idx`, then tonemap into `target`.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, src_idx: usize) {
        let c = &self.chain;
        let draw = |encoder: &mut wgpu::CommandEncoder,
                    pipeline: &wgpu::RenderPipeline,
                    step: &BloomStep,
                    view: &wgpu::TextureView,
                    load: wgpu::LoadOp<wgpu::Color>| {
            let mut rpass = gpu::color_pass(encoder, "seoul.bloom", view, load);
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &step.bind_group, &[]);
            rpass.draw(0..3, 0..1);
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);

        draw(encoder, &self.bloom_prefilter, &c.prefilter[src_idx], &c.mip_views[0], clear);
        for (i, step) in c.down.iter().enumerate() {
            draw(encoder, &self.bloom_down, step, &c.mip_views[i + 1], clear);
        }
        for (i, step) in c.up.iter().enumerate().rev() {
            draw(encoder, &self.bloom_up, step, &c.mip_views[i], wgpu::LoadOp::Load);
        }

        let mut rpass = gpu::color_pass(encoder, "seoul.post.final", target, clear);
        rpass.set_pipeline(&self.final_pipeline);
        rpass.set_bind_group(0, &self.final_tex_bgs[src_idx], &[]);
        rpass.set_bind_group(1, &self.final_uniform_bg, &[]);
        rpass.draw(0..3, 0..1);
    }
}

fn build_chain(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    feedback_views: &[wgpu::TextureView; 2],
    feedback_size: (u32, u32),
) -> BloomChain {
    let base = ((feedback_size.0 / 2).max(1), (feedback_size.1 / 2).max(1));
    // Stop before the smallest mip drops under ~8 px.
    let mut levels = 1;
    while levels < BLOOM_MAX_LEVELS && (base.0.min(base.1) >> levels) >= 8 {
        levels += 1;
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("seoul.bloom.chain"),
        size: wgpu::Extent3d {
            width: base.0,
            height: base.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: BLOOM_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mip_views: Vec<wgpu::TextureView> = (0..levels)
        .map(|m| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("seoul.bloom.mip"),
                base_mip_level: m,
                mip_level_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let mip_size = |m: u32| ((base.0 >> m).max(1), (base.1 >> m).max(1));

    let step = |src: &wgpu::TextureView, src_size: (u32, u32)| {
        let u = BloomUniforms {
            texel: [1.0 / src_size.0 as f32, 1.0 / src_size.1 as f32],
            threshold: 0.0,
            knee: 0.1,
        };
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.bloom.uniforms"),
            contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.bloom.bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        BloomStep { bind_group, uniforms }
    };

    let prefilter = [
        step(&feedback_views[0], feedback_size),
        step(&feedback_views[1], feedback_size),
    ];
    let down = (0..levels - 1)
        .map(|m| step(&mip_views[m as usize], mip_size(m)))
        .collect();
    let up = (0..levels - 1)
        .map(|m| step(&mip_views[m as usize + 1], mip_size(m + 1)))
        .collect();

    BloomChain {
        _texture: texture,
        mip_views,
        prefilter,
        down,
        up,
    }
}

fn build_final_bgs(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    feedback_views: &[wgpu::TextureView; 2],
    chain: &BloomChain,
) -> [wgpu::BindGroup; 2] {
    let make = |scene: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.post.tex.bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&chain.mip_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    };
    [make(&feedback_views[0]), make(&feedback_views[1])]
}
