//! Ping-pong HDR textures that carry the image from frame to frame.

use tracing::info;

use crate::render::gpu;

pub const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Upper bound on either feedback dimension (4K-class), whatever the window.
pub const MAX_FEEDBACK_DIM: u32 = 4096;

pub struct FeedbackTextures {
    // Kept alive to back `views`.
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    size: (u32, u32),
    current_read: usize,
}

impl FeedbackTextures {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) -> Self {
        let (w, h) = clamp_size(size);
        let make_tex = |label: &'static str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FEEDBACK_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };

        let tex_a = make_tex("seoul.feedback.a");
        let tex_b = make_tex("seoul.feedback.b");
        let view_a = tex_a.create_view(&Default::default());
        let view_b = tex_b.create_view(&Default::default());

        // Clear both so first-frame sampling reads known pixels.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.feedback.init_clear"),
        });
        for view in [&view_a, &view_b] {
            let _ = gpu::color_pass(
                &mut encoder,
                "seoul.feedback.clear",
                view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            );
        }
        queue.submit(Some(encoder.finish()));

        info!(width = w, height = h, format = ?FEEDBACK_FORMAT, "feedback textures ready");

        Self {
            _textures: [tex_a, tex_b],
            views: [view_a, view_b],
            size: (w, h),
            current_read: 0,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn views(&self) -> &[wgpu::TextureView; 2] {
        &self.views
    }

    pub fn read_index(&self) -> usize {
        self.current_read
    }

    pub fn write_index(&self) -> usize {
        self.current_read ^ 1
    }

    pub fn read_view(&self) -> &wgpu::TextureView {
        &self.views[self.read_index()]
    }

    pub fn write_view(&self) -> &wgpu::TextureView {
        &self.views[self.write_index()]
    }

    pub fn swap(&mut self) {
        self.current_read ^= 1;
    }
}

/// Feedback size for a window of `size`, scaled and clamped.
pub fn feedback_size(window: (u32, u32), render_scale: f32) -> (u32, u32) {
    let s = render_scale.clamp(0.25, 2.0);
    clamp_size((
        (window.0 as f32 * s).round() as u32,
        (window.1 as f32 * s).round() as u32,
    ))
}

fn clamp_size((w, h): (u32, u32)) -> (u32, u32) {
    let (w, h) = (w.max(16), h.max(16));
    let over = (w.max(h) as f32 / MAX_FEEDBACK_DIM as f32).max(1.0);
    ((w as f32 / over) as u32, (h as f32 / over) as u32)
}
