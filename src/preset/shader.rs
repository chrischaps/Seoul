//! Composite shader composition + pipeline factory for presets.
//!
//! Each preset author writes only `fs_composite`. We prepend a shared prelude
//! (bindings, Varying, vs_fullscreen, helpers) before compilation so every
//! preset shader has the same boilerplate.
//!
//! Both shader-module and pipeline creation run inside wgpu validation error
//! scopes, so a broken preset — bad syntax, a missing entry point, a wrong
//! signature — comes back as `Err` instead of panicking the device. That is
//! what makes hot-reload safe. Error line numbers are remapped from the
//! combined prelude+body source back to the author's file.

use anyhow::{Result, anyhow};
use wgpu::util::DeviceExt;

use crate::preset::Palette;
use crate::render::feedback::FEEDBACK_FORMAT;

pub const COMPOSITE_PRELUDE: &str = include_str!("../../shaders/composite_prelude.wgsl");

/// Bind group layouts shared across every composite pipeline.
pub struct CompositeLayouts {
    pub palette_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
}

impl CompositeLayouts {
    pub fn new(device: &wgpu::Device, audio_layout: &wgpu::BindGroupLayout) -> Self {
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
            bind_group_layouts: &[Some(audio_layout), Some(&palette_layout)],
            immediate_size: 0,
        });

        Self {
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

/// Prepend `prelude` to an author's WGSL `body` and compile it. `file` names
/// the author's file in error messages.
pub fn compile_wgsl(
    device: &wgpu::Device,
    prelude: &str,
    body: &str,
    label: &str,
    file: &str,
) -> Result<wgpu::ShaderModule> {
    let mut combined = String::with_capacity(prelude.len() + body.len() + 1);
    combined.push_str(prelude);
    combined.push('\n');
    let body_offset = combined.matches('\n').count();
    combined.push_str(body);

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(combined.into()),
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(anyhow!("{}", remap_lines(&e.to_string(), body_offset, file)));
    }
    Ok(module)
}

/// Read a preset's WGSL fragment, prepend the composite prelude, and compile.
pub fn compile_composite_shader(
    device: &wgpu::Device,
    body_source: &str,
    label_hint: &str,
    file: &str,
) -> Result<wgpu::ShaderModule> {
    compile_wgsl(
        device,
        COMPOSITE_PRELUDE,
        body_source,
        &format!("seoul.composite.shader.{label_hint}"),
        file,
    )
}

/// Build a composite render pipeline using the given (already compiled) shader.
pub fn build_composite_pipeline(
    device: &wgpu::Device,
    layouts: &CompositeLayouts,
    shader: &wgpu::ShaderModule,
    label_hint: &str,
) -> Result<wgpu::RenderPipeline> {
    // Additive, scaled by the per-draw blend constant (transition intensity
    // × frame-time normalization).
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };

    let label = format!("seoul.composite.pipeline.{label_hint}");
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(anyhow!("pipeline for '{label_hint}' failed: {e}"));
    }
    Ok(pipeline)
}

/// Rewrite naga's `wgsl:LINE:COL` locations and `LINE │` gutters so they
/// point into the author's file rather than the combined source.
fn remap_lines(msg: &str, offset: usize, file: &str) -> String {
    let fix = |n: usize| -> (String, bool) {
        if n > offset {
            ((n - offset).to_string(), true)
        } else {
            (n.to_string(), false)
        }
    };

    let mut out = String::with_capacity(msg.len());
    for (li, line) in msg.lines().enumerate() {
        if li > 0 {
            out.push('\n');
        }
        // Gutter: optional spaces, a line number, a space, then '│'.
        let trimmed = line.trim_start();
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && trimmed[digits.len()..].starts_with(" │") {
            let indent = &line[..line.len() - trimmed.len()];
            let (n, _) = fix(digits.parse().unwrap_or(0));
            out.push_str(indent);
            out.push_str(&format!("{n:>width$}", width = digits.len()));
            out.push_str(&trimmed[digits.len()..]);
            continue;
        }

        let mut rest = line;
        while let Some(pos) = rest.find("wgsl:") {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 5..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                out.push_str("wgsl:");
                rest = after;
                continue;
            }
            let (n, in_body) = fix(digits.parse().unwrap_or(0));
            out.push_str(if in_body { file } else { "prelude" });
            out.push(':');
            out.push_str(&n);
            rest = &after[digits.len()..];
        }
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::remap_lines;

    #[test]
    fn remaps_locations_and_gutters_into_body() {
        let msg = "error: expected ';'\n   ┌─ wgsl:83:5\n   │\n83 │     let x =\n   │     ^^^";
        let out = remap_lines(msg, 80, "presets/x.wgsl");
        assert!(out.contains("presets/x.wgsl:3:5"), "{out}");
        assert!(out.contains(" 3 │     let x ="), "{out}");
    }

    #[test]
    fn prelude_locations_are_labelled() {
        let out = remap_lines("at wgsl:10:1", 80, "x.wgsl");
        assert_eq!(out, "at prelude:10:1");
    }
}
