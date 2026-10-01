use tracing::info;

pub const FEEDBACK_WIDTH: u32 = 1280;
pub const FEEDBACK_HEIGHT: u32 = 720;
pub const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub struct FeedbackTextures {
    // Kept alive to back `views`; dropping would release the GPU textures.
    #[allow(dead_code)]
    pub textures: [wgpu::Texture; 2],
    pub views: [wgpu::TextureView; 2],
    #[allow(dead_code)]
    pub size: (u32, u32),
    current_read: usize,
}

impl FeedbackTextures {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let size = (FEEDBACK_WIDTH, FEEDBACK_HEIGHT);
        let make_tex = |label: &'static str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: FEEDBACK_WIDTH,
                    height: FEEDBACK_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FEEDBACK_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };

        let tex_a = make_tex("seoul.feedback.a");
        let tex_b = make_tex("seoul.feedback.b");
        let view_a = tex_a.create_view(&wgpu::TextureViewDescriptor::default());
        let view_b = tex_b.create_view(&wgpu::TextureViewDescriptor::default());

        // Explicitly clear both textures to black so first-frame sampling reads
        // known pixels (wgpu does not guarantee zero-init for render targets).
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.feedback.init_clear"),
        });
        for view in [&view_a, &view_b] {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("seoul.feedback.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
        }
        queue.submit(Some(encoder.finish()));

        info!(
            width = FEEDBACK_WIDTH,
            height = FEEDBACK_HEIGHT,
            format = ?FEEDBACK_FORMAT,
            "feedback textures initialized"
        );

        Self {
            textures: [tex_a, tex_b],
            views: [view_a, view_b],
            size,
            current_read: 0,
        }
    }

    pub fn read_index(&self) -> usize {
        self.current_read
    }

    pub fn write_index(&self) -> usize {
        self.current_read ^ 1
    }

    pub fn write_view(&self) -> &wgpu::TextureView {
        &self.views[self.write_index()]
    }

    pub fn swap(&mut self) {
        self.current_read ^= 1;
    }
}
