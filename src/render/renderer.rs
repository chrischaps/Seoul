use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;

use crate::audio::AudioFeatures;
use crate::preset::PresetLibrary;
use crate::preset::shader::CompositeLayouts;
use crate::render::blit::BlitPass;
use crate::render::context::RenderContext;
use crate::render::feedback::FeedbackTextures;
use crate::render::mesh::WarpMesh;
use crate::render::warp::WarpPass;

pub struct Renderer {
    audio_buf: wgpu::Buffer,
    audio_bg: wgpu::BindGroup,
    feedback: FeedbackTextures,
    mesh: WarpMesh,
    warp: WarpPass,
    blit: BlitPass,
    library: PresetLibrary,
    last_render_time: Option<Instant>,
}

impl Renderer {
    pub fn new(ctx: &RenderContext, presets_dir: &Path) -> Result<Self> {
        let device = &ctx.device;

        let initial = AudioFeatures::default();
        let audio_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.audio_features"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let audio_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.audio_features.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let audio_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("seoul.audio_features.bg"),
            layout: &audio_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: audio_buf.as_entire_binding(),
            }],
        });

        let feedback = FeedbackTextures::new(device, &ctx.queue);
        let mesh = WarpMesh::new(device);
        let warp = WarpPass::new(device, &feedback.views);
        let blit = BlitPass::new(device, ctx.config.format, &feedback.views, ctx.size);

        let composite_layouts = CompositeLayouts::new(device, audio_layout);
        let library = PresetLibrary::load(device, presets_dir, composite_layouts)?;

        Ok(Self {
            audio_buf,
            audio_bg,
            feedback,
            mesh,
            warp,
            blit,
            library,
            last_render_time: None,
        })
    }

    pub fn library_mut(&mut self) -> &mut PresetLibrary {
        &mut self.library
    }

    pub fn resize(&self, queue: &wgpu::Queue, size: PhysicalSize<u32>) {
        self.blit.update_resolution(queue, size);
    }

    pub fn render(
        &mut self,
        ctx: &RenderContext,
        swap_view: &wgpu::TextureView,
        features: &AudioFeatures,
    ) {
        // Compute frame dt
        let now = Instant::now();
        let dt = match self.last_render_time.replace(now) {
            Some(prev) => (now - prev).as_secs_f32().min(0.1),
            None => 0.0,
        };

        // Hot-reload any preset changes before evaluating.
        self.library.poll_reloads(&ctx.device);

        // Advance preset state + evaluate this frame
        self.library.tick(features, dt);
        let plan = self.library.frame_plan(features);

        // Upload audio + mesh UVs + warp decay
        ctx.queue
            .write_buffer(&self.audio_buf, 0, bytemuck::bytes_of(features));
        self.mesh.update(&plan.warp, features.time);
        self.mesh.upload(&ctx.queue);
        self.warp.update_decay(&ctx.queue, plan.decay);

        let read_idx = self.feedback.read_index();
        let write_idx = self.feedback.write_index();
        let write_view = self.feedback.write_view();

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("seoul.frame"),
            });

        // Pass 1: Warp — read prev feedback, write current
        self.warp
            .render(&mut encoder, write_view, read_idx, &self.mesh);

        // Pass 2: Composite — one or two draws, additive with per-call blend constant
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("seoul.composite.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: write_view,
                    resolve_target: None,
                depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            multiview_mask: None,
            });
            for draw in &plan.draws {
                let i = draw.intensity;
                rpass.set_blend_constant(wgpu::Color {
                    r: i as f64,
                    g: i as f64,
                    b: i as f64,
                    a: i as f64,
                });
                rpass.set_pipeline(draw.pipeline);
                rpass.set_bind_group(0, &self.audio_bg, &[]);
                rpass.set_bind_group(1, draw.palette_bg, &[]);
                rpass.draw(0..3, 0..1);
            }
        }

        // Pass 3: Blit — copy feedback to swapchain with letterbox
        self.blit.render(&mut encoder, swap_view, write_idx);

        ctx.queue.submit(Some(encoder.finish()));
        self.feedback.swap();
    }
}
