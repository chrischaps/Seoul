//! Composite shader composition + pipeline factory for presets.
//!
//! Each preset author writes only `fs_composite`. We prepend a shared prelude
//! (bindings, Varying, vs_fullscreen, helpers) before compilation so every
//! preset shader has the same boilerplate.

use anyhow::Result;
use wgpu::util::DeviceExt;

use crate::preset::Palette;
use crate::render::feedback::FEEDBACK_FORMAT;

const PRELUDE: &str = include_str!("../../shaders/composite_prelude.wgsl");

/// Bind group layouts shared across every composite pipeline.
pub struct CompositeLayouts {
    pub audio_layout: wgpu::BindGroupLayout,
    pub palette_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
}

impl CompositeLayouts {
    pub fn new(device: &wgpu::Device, audio_layout: wgpu::BindGroupLayout) -> Self {
        let palette_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.composite.palette.bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("seoul.composite.pipeline_layout"),
            bind_group_layouts: &[Some(&audio_layout), Some(&palette_layout)],
            immediate_size: 0,
        });

        Self {
            audio_layout,
            palette_layout,
            pipeline_layout,
        }
    }
}

/// Create a palette uniform buffer initialized to a given Palette,
/// plus its bind group.
pub fn make_palette_resources(
    device: &wgpu::Device,
    layouts: &CompositeLayouts,
    palette: &Palette,
    label_hint: &str,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buf_label = format!("seoul.preset.palette.{label_hint}");
    let bg_label = format!("seoul.preset.palette_bg.{label_hint}");
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&buf_label),
        contents: bytemuck::bytes_of(palette),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&bg_label),
        layout: &layouts.palette_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    (buf, bg)
}

/// Read a preset's WGSL fragment, prepend the prelude, and compile.
/// Returns the ShaderModule on success or a wgpu validation error.
pub fn compile_composite_shader(
    device: &wgpu::Device,
    body_source: &str,
    label_hint: &str,
) -> Result<wgpu::ShaderModule> {
    let combined = compose_source(PRELUDE, body_source);
    let label = format!("seoul.composite.shader.{label_hint}");

    // We push our own validation error scope so a bad preset shader returns
    // an error instead of panicking the device.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&label),
        source: wgpu::ShaderSource::Wgsl(combined.into()),
    });
    let err = pollster::block_on(scope.pop());
    if let Some(e) = err {
        return Err(anyhow::anyhow!(
            "shader compile failed for '{label_hint}': {e}"
        ));
    }
    Ok(module)
}

/// Build a composite render pipeline using the given (already compiled) shader.
pub fn build_composite_pipeline(
    device: &wgpu::Device,
    layouts: &CompositeLayouts,
    shader: &wgpu::ShaderModule,
    label_hint: &str,
) -> wgpu::RenderPipeline {
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };

    let label = format!("seoul.composite.pipeline.{label_hint}");
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&label),
        layout: Some(&layouts.pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_composite"),
            targets: &[Some(wgpu::ColorTargetState {
                format: FEEDBACK_FORMAT,
                blend: Some(blend),
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
    })
}

fn compose_source(prelude: &str, body: &str) -> String {
    let mut s = String::with_capacity(prelude.len() + body.len() + 2);
    s.push_str(prelude);
    s.push('\n');
    s.push_str(body);
    s
}
