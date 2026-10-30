use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;
use winit::dpi::PhysicalSize;
use winit::window::Window;

pub struct RenderContext {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: PhysicalSize<u32>,
    /// The GPU and backend, as "name (Vulkan)".
    pub adapter: String,
    // Kept alive to anchor the surface's 'static lifetime.
    #[allow(dead_code)]
    window: Arc<Window>,
}

impl RenderContext {
    /// Without `vsync` (`--bench`), frames present as fast as the GPU
    /// draws them, where the surface allows it.
    pub async fn new(window: Arc<Window>, vsync: bool) -> Result<Self> {
        let size = window.inner_size();
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // WGPU_BACKEND=gl tries OpenGL ES, for GPUs whose Vulkan falls short.
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("create_surface failed")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no compatible GPU adapter")?;

        let adapter_name = format!("{} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        info!(
            adapter = %adapter.get_info().name,
            backend = ?adapter.get_info().backend,
            "wgpu adapter selected"
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("seoul-device"),
                required_features: wgpu::Features::empty(),
                // The default limits, lowered where the GPU falls short: the
                // Raspberry Pi 5's V3D passes 15 inter-stage variables, not 16.
                required_limits: wgpu::Limits::default().or_worse_values_from(&adapter.limits()),
                ..Default::default()
            })
            .await
            .context("request_device failed")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width,
            height: size.height,
            present_mode: if vsync { wgpu::PresentMode::Fifo } else { wgpu::PresentMode::AutoNoVsync },
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        info!(width = size.width, height = size.height, ?format, "surface configured");

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            adapter: adapter_name,
            window,
        })
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
    }
}
