//! Frame orchestration: warp → particles → composite → bloom → post.

use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;

use crate::audio::AudioFeatures;
use crate::preset::PresetLibrary;
use crate::preset::curation::Curation;
use crate::preset::library::{LibrarySettings, PresetLayouts};
use crate::preset::shader::{CompositeLayouts, palette_layout};
use crate::render::context::RenderContext;
use crate::render::feedback::{FeedbackTextures, feedback_size};
use crate::render::gpu;
use crate::render::hud::{Hud, HudInput};
use crate::render::mask::{MaskPass, MaskSlot};
use crate::render::particles::{ParticleFrame, ParticleSystem};
use crate::render::post::PostPass;
use crate::render::screenshot::CaptureTarget;
use crate::render::warp::{WarpDraw, WarpLayouts, WarpPass};

pub struct RendererOptions {
    pub render_scale: f32,
    pub library: LibrarySettings,
    pub curation: Curation,
    /// Window DPI scale for the HUD.
    pub ui_scale: f64,
}

pub struct Renderer {
    audio_buf: wgpu::Buffer,
    audio_bg: wgpu::BindGroup,
    feedback: FeedbackTextures,
    warp: WarpPass,
    mask: MaskPass,
    particles: ParticleSystem,
    post: PostPass,
    hud: Hud,
    library: PresetLibrary,
    surface_format: wgpu::TextureFormat,
    output_size: (u32, u32),
    render_scale: f32,
    start_time: Instant,
    last_render_time: Option<Instant>,
    frame: u64,
    /// Offline rendering: advance exactly this much per frame instead of
    /// following the wall clock.
    fixed_dt: Option<f32>,
    sim_time: f32,
}

impl Renderer {
    pub fn new(ctx: &RenderContext, presets_dir: &Path, opts: RendererOptions) -> Result<Self> {
        let device = &ctx.device;

        let audio_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.audio_features"),
            contents: bytemuck::bytes_of(&AudioFeatures::default()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // Audio features are read by composite and warp fragments, particle
        // vertices and the particle simulation.
        let audio_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("seoul.audio_features.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
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

        let output_size = (ctx.size.width, ctx.size.height);
        let fb_size = feedback_size(output_size, opts.render_scale);
        let palette_layout = palette_layout(device);
        let warp_layouts = WarpLayouts::new(device, &audio_layout, &palette_layout);

        let feedback = FeedbackTextures::new(device, &ctx.queue, fb_size);
        let warp = WarpPass::new(device, feedback.views(), warp_layouts.clone())?;
        let mask = MaskPass::new(device);
        let particles = ParticleSystem::new(device, &audio_layout, &palette_layout)?;
        let post = PostPass::new(device, ctx.config.format, feedback.views(), feedback.size());
        let hud = Hud::new(device, &ctx.queue, ctx.config.format, opts.ui_scale);

        let layouts = PresetLayouts {
            composite: CompositeLayouts::new(device, &audio_layout, &palette_layout),
            warp: warp_layouts,
        };
        let library = PresetLibrary::load(device, presets_dir, layouts, opts.library, opts.curation)?;

        Ok(Self {
            audio_buf,
            audio_bg,
            feedback,
            warp,
            mask,
            particles,
            post,
            hud,
            library,
            surface_format: ctx.config.format,
            output_size,
            render_scale: opts.render_scale,
            start_time: Instant::now(),
            last_render_time: None,
            frame: 0,
            fixed_dt: None,
            sim_time: 0.0,
        })
    }

    pub fn library(&self) -> &PresetLibrary {
        &self.library
    }

    pub fn library_mut(&mut self) -> &mut PresetLibrary {
        &mut self.library
    }

    pub fn hud_mut(&mut self) -> &mut Hud {
        &mut self.hud
    }

    /// Switch to a fixed timestep (offline recording) or back to real time.
    pub fn set_fixed_dt(&mut self, dt: Option<f32>) {
        self.fixed_dt = dt;
    }


    /// Track the window size. The feedback loop follows it (× render scale);
    /// the current image is resampled into the new textures so a resize
    /// doesn't flash to black.
    pub fn resize(&mut self, ctx: &RenderContext, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.output_size = (size.width, size.height);
        let fb_size = feedback_size(self.output_size, self.render_scale);
        if fb_size == self.feedback.size() {
            return;
        }

        let device = &ctx.device;
        let fresh = FeedbackTextures::new(device, &ctx.queue, fb_size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.feedback.resize"),
        });
        self.warp.resample(
            device,
            &mut encoder,
            self.feedback.read_view(),
            fresh.read_view(),
            &self.audio_bg,
        );
        ctx.queue.submit(Some(encoder.finish()));

        self.feedback = fresh;
        self.warp.rebind(device, self.feedback.views());
        self.post.rebind(device, self.feedback.views(), self.feedback.size());
    }

    pub fn render(&mut self, ctx: &RenderContext, swap_view: &wgpu::TextureView, features: &AudioFeatures) {
        // The render thread owns the clock: audio analysis can stall (WASAPI
        // loopback sends nothing during silence) but visuals must keep moving.
        let now = Instant::now();
        let (dt, time) = match self.fixed_dt {
            Some(step) => {
                self.sim_time += step;
                (step, self.sim_time)
            }
            None => {
                let dt = match self.last_render_time.replace(now) {
                    Some(prev) => (now - prev).as_secs_f32().min(0.1),
                    None => 1.0 / 60.0,
                };
                (dt, (now - self.start_time).as_secs_f32())
            }
        };
        let (fw, fh) = self.feedback.size();
        let mut f = *features;
        f.time = time;
        f.dt = dt;
        f.frame = self.frame as f32;
        f.resolution = [fw as f32, fh as f32];
        f.aspect = fw as f32 / fh as f32;
        self.frame += 1;

        // Hot-reload any preset changes before evaluating.
        self.library.poll_reloads(&ctx.device);

        self.library.tick(&f, dt);
        let plan = self.library.frame_plan(&f);

        ctx.queue.write_buffer(&self.audio_buf, 0, bytemuck::bytes_of(&f));
        self.post.update(&ctx.queue, &plan.post, f.time, f.beat, self.output_size);
        let warp_draws: Vec<WarpDraw> = plan
            .warps
            .iter()
            .map(|w| WarpDraw {
                pipeline: w.pipeline,
                uniforms: w.params.to_uniforms(dt, f.time, f.aspect, (fw, fh)),
                palette_bg: w.palette_bg,
            })
            .collect();

        let read_idx = self.feedback.read_index();
        let write_idx = self.feedback.write_index();
        let write_view = self.feedback.write_view();

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.frame"),
        });

        // Per-pixel weights for each preset's draws (transition masks).
        let k = (dt * 60.0).clamp(0.0, 6.0);
        self.mask.prepare(&ctx.queue, plan.transition, k, f.aspect);

        // Pass 1: warp last frame into this one — one masked draw per active
        // preset, so transitions blend the feedback dynamics themselves.
        self.warp.render(
            &ctx.queue,
            &mut encoder,
            write_view,
            read_idx,
            &self.audio_bg,
            &self.mask,
            &warp_draws,
        );

        // Pass 1b: particles add straight into the feedback, leaving trails.
        if let Some((pplan, palette_bg)) = &plan.particles {
            self.particles.run(
                &ctx.queue,
                &mut encoder,
                write_view,
                pplan,
                &ParticleFrame {
                    dt,
                    time: f.time,
                    frame: self.frame as u32,
                    aspect: f.aspect,
                    resolution: (fw, fh),
                    beat_count: f.beat_count,
                    audio_bg: &self.audio_bg,
                    palette_bg,
                },
            );
        }

        // Pass 2: composite — additive, one draw per active preset, weighted
        // by its mask (which also carries frame-time normalization, so
        // accumulated brightness doesn't depend on refresh rate).
        {
            let mut rpass = gpu::color_pass(&mut encoder, "seoul.composite.pass", write_view, wgpu::LoadOp::Load);
            for (i, draw) in plan.draws.iter().enumerate() {
                self.mask.draw(&mut rpass, MaskSlot::composite(i));
                rpass.set_pipeline(draw.pipeline);
                rpass.set_bind_group(0, &self.audio_bg, &[]);
                rpass.set_bind_group(1, draw.palette_bg, &[]);
                rpass.draw(0..3, 0..1);
            }
        }

        // Pass 3: bloom + tonemap into the swapchain.
        self.post.render(&mut encoder, swap_view, write_idx);

        // Pass 4: HUD over the finished frame.
        let lib = &self.library;
        let input = HudInput {
            features: &f,
            preset: lib.current(),
            favorite: lib.is_favorite(),
            locked: lib.is_locked(),
            auto: lib.auto_advance(),
            transition: lib.transition_progress(),
            error: lib.last_error(),
        };
        self.hud.prepare(&ctx.device, &ctx.queue, self.output_size, dt, &input);
        self.hud.render(&mut encoder, swap_view);

        ctx.queue.submit(Some(encoder.finish()));
        self.hud.trim();
        self.feedback.swap();
    }

    /// Re-run the display passes for the last rendered frame into an
    /// offscreen target and save it as PNG.
    pub fn screenshot(&self, ctx: &RenderContext, path: &Path) -> Result<()> {
        let target = CaptureTarget::new(&ctx.device, self.surface_format, self.output_size);
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.screenshot"),
        });
        // After `swap`, the read texture is the one this frame wrote. The
        // HUD is still prepared from that frame, so it lands in the shot too.
        self.post.render(&mut encoder, &target.view, self.feedback.read_index());
        self.hud.render(&mut encoder, &target.view);
        ctx.queue.submit(Some(encoder.finish()));
        target.save_png(&ctx.device, &ctx.queue, path)
    }
}
