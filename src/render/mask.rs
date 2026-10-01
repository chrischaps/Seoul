//! Per-pixel transition weights via the feedback alpha channel.
//!
//! Before each preset's warp and composite draw, an alpha-only fullscreen
//! pass writes that preset's weight; those pipelines blend with
//! `src × DstAlpha`. Stable frames use the same path with a uniform weight.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::preset::transition::TransitionStyle;
use crate::render::feedback::FEEDBACK_FORMAT;
use crate::render::gpu;

/// What the active transition looks like this frame.
#[derive(Copy, Clone, Debug)]
pub struct TransitionMask {
    /// Eased progress, 0..1.
    pub progress: f32,
    pub style: TransitionStyle,
    pub seed: f32,
}

/// Which mask a draw needs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MaskSlot {
    WarpFrom = 0,
    WarpTo = 1,
    CompositeFrom = 2,
    CompositeTo = 3,
}

impl MaskSlot {
    /// Slot for draw `i` of a frame's warp or composite list.
    pub fn warp(i: usize) -> Self {
        if i == 0 { Self::WarpFrom } else { Self::WarpTo }
    }
    pub fn composite(i: usize) -> Self {
        if i == 0 { Self::CompositeFrom } else { Self::CompositeTo }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct MaskUniforms {
    progress: f32,
    style: f32,
    side: f32,
    scale: f32,
    seed: f32,
    aspect: f32,
    _pad: [f32; 2],
}

struct Slot {
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

pub struct MaskPass {
    pipeline: wgpu::RenderPipeline,
    slots: [Slot; 4],
}

impl MaskPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = gpu::shader_module(device, "seoul.mask.wgsl", include_str!("../../shaders/mask.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.mask.bgl"),
            entries: &[gpu::uniform_entry(0)],
        });
        let make = |i: usize| {
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("seoul.mask.{i}")),
                contents: bytemuck::bytes_of(&MaskUniforms::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("seoul.mask.bg"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            Slot { buf, bg }
        };
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.mask.layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("seoul.mask.pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mask"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mask"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FEEDBACK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALPHA,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            slots: [make(0), make(1), make(2), make(3)],
        }
    }

    /// Write this frame's four masks. `k` is the composite's frame-time
    /// normalization (warps are already normalized in their uniforms).
    pub fn prepare(&self, queue: &wgpu::Queue, transition: Option<TransitionMask>, k: f32, aspect: f32) {
        for (i, slot) in self.slots.iter().enumerate() {
            let incoming = i % 2 == 1;
            let scale = if i >= 2 { k } else { 1.0 };
            let u = match transition {
                None => MaskUniforms {
                    side: 2.0,
                    scale,
                    aspect,
                    ..Zeroable::zeroed()
                },
                Some(t) => MaskUniforms {
                    progress: t.progress,
                    style: t.style.as_f32(),
                    side: if incoming { 1.0 } else { 0.0 },
                    scale,
                    seed: t.seed,
                    aspect,
                    _pad: [0.0; 2],
                },
            };
            queue.write_buffer(&slot.buf, 0, bytemuck::bytes_of(&u));
        }
    }

    /// Write mask `slot` into the alpha of the pass's target.
    pub fn draw(&self, rpass: &mut wgpu::RenderPass, slot: MaskSlot) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.slots[slot as usize].bg, &[]);
        rpass.draw(0..3, 0..1);
    }
}

/// Blend for anything weighted by the mask: `dst += src × dst.alpha`, color
/// channels only so the mask survives until the next mask pass.
pub const MASKED_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::DstAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::REPLACE,
};
