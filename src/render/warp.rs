use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::render::feedback::FEEDBACK_FORMAT;
use crate::render::mesh::WarpMesh;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct WarpUniforms {
    decay: f32,
    _pad: [f32; 3],
}

pub struct WarpPass {
    pipeline: wgpu::RenderPipeline,
    texture_bgs: [wgpu::BindGroup; 2],
    decay_buf: wgpu::Buffer,
    decay_bg: wgpu::BindGroup,
    _sampler: wgpu::Sampler,
}

impl WarpPass {
    pub fn new(device: &wgpu::Device, feedback_views: &[wgpu::TextureView; 2]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("seoul.warp.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/warp.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("seoul.warp.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.tex.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let make_bg = |view: &wgpu::TextureView, label: &'static str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let texture_bgs = [
            make_bg(&feedback_views[0], "seoul.warp.tex.bg.0"),
            make_bg(&feedback_views[1], "seoul.warp.tex.bg.1"),
        ];

        let decay_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.warp.decay.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let initial = WarpUniforms {
            decay: 0.97,
            _pad: [0.0; 3],
        };
        let decay_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.warp.decay.buf"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let decay_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.warp.decay.bg"),
            layout: &decay_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: decay_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.warp.layout"),
            bind_group_layouts: &[Some(&texture_layout), Some(&decay_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("seoul.warp.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_warp"),
                buffers: &[Some(WarpMesh::vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_warp"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FEEDBACK_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
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

        Self {
            pipeline,
            texture_bgs,
            decay_buf,
            decay_bg,
            _sampler: sampler,
        }
    }

    pub fn update_decay(&self, queue: &wgpu::Queue, decay: f32) {
        let u = WarpUniforms {
            decay,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.decay_buf, 0, bytemuck::bytes_of(&u));
    }

    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        read_idx: usize,
        mesh: &WarpMesh,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("seoul.warp.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.texture_bgs[read_idx], &[]);
        rpass.set_bind_group(1, &self.decay_bg, &[]);
        rpass.set_vertex_buffer(0, mesh.vbuf.slice(..));
        rpass.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint16);
        rpass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}
