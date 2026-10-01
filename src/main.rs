use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info};
use triple_buffer::Output;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

mod audio;
mod preset;
mod render;

use std::path::PathBuf;

use crate::audio::AudioFeatures;
use crate::audio::capture::LoopbackCapture;
use crate::render::{RenderContext, Renderer};

struct App {
    window: Option<Arc<Window>>,
    ctx: Option<RenderContext>,
    renderer: Option<Renderer>,
    features: Output<AudioFeatures>,
    capture: LoopbackCapture,
    fullscreen: bool,
    reconfigure: bool,
}

impl App {
    fn toggle_fullscreen(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        window.set_fullscreen(if self.fullscreen {
            Some(Fullscreen::Borderless(None))
        } else {
            None
        });
    }

    fn render(&mut self) {
        self.capture.poll();

        let (Some(ctx), Some(renderer), Some(window)) = (
            self.ctx.as_mut(),
            self.renderer.as_mut(),
            self.window.as_ref(),
        ) else {
            return;
        };

        let features = *self.features.read();

        let frame = match ctx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                // Render this frame, reconfigure before the next one.
                self.reconfigure = true;
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                ctx.surface.configure(&ctx.device, &ctx.config);
                window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                window.request_redraw();
                return;
            }
            other => {
                error!(?other, "surface acquire failed");
                window.request_redraw();
                return;
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render(ctx, &view, &features);
        ctx.queue.present(frame);
        if std::mem::take(&mut self.reconfigure) {
            ctx.surface.configure(&ctx.device, &ctx.config);
        }

        window.request_redraw();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Seoul")
            .with_inner_size(LogicalSize::new(1280, 720));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let ctx = pollster::block_on(RenderContext::new(window.clone()))
            .expect("failed to create render context");
        let presets_dir = PathBuf::from("presets");
        let renderer = Renderer::new(&ctx, &presets_dir)
            .expect("failed to load presets");

        window.request_redraw();
        self.window = Some(window);
        self.ctx = Some(ctx);
        self.renderer = Some(renderer);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(ctx) = self.ctx.as_mut() {
                    ctx.resize(size);
                }
                if let (Some(ctx), Some(renderer)) = (self.ctx.as_ref(), self.renderer.as_ref()) {
                    renderer.resize(&ctx.queue, size);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(kc),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => match kc {
                KeyCode::Escape => event_loop.exit(),
                KeyCode::F11 => self.toggle_fullscreen(),
                KeyCode::Space => {
                    if let Some(r) = self.renderer.as_mut() {
                        r.library_mut().next();
                        info!(preset = r.library_mut().current_name(), "next");
                    }
                }
                KeyCode::Backspace => {
                    if let Some(r) = self.renderer.as_mut() {
                        r.library_mut().prev();
                        info!(preset = r.library_mut().current_name(), "prev");
                    }
                }
                KeyCode::KeyR => {
                    if let Some(r) = self.renderer.as_mut() {
                        r.library_mut().random();
                        info!(preset = r.library_mut().current_name(), "random");
                    }
                }
                KeyCode::KeyA => {
                    if let Some(r) = self.renderer.as_mut() {
                        let on = r.library_mut().toggle_auto_advance();
                        info!(auto_advance = on, "auto-advance");
                    }
                }
                _ => {}
            },
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "seoul=info,wgpu_core=warn,wgpu_hal=warn".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("seoul starting");

    let (features, source_tx) = audio::analysis::spawn_analysis();
    let capture = LoopbackCapture::new(source_tx);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        window: None,
        ctx: None,
        renderer: None,
        features,
        capture,
        fullscreen: false,
        reconfigure: false,
    };

    event_loop.run_app(&mut app)?;
    Ok(())
}
