use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tracing::{error, info, warn};
use triple_buffer::Output;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

mod audio;
mod cli;
mod preset;
mod render;

use crate::audio::AudioFeatures;
use crate::audio::capture::LoopbackCapture;
use crate::cli::Args;
use crate::render::post::PostParams;
use crate::render::{RenderContext, Renderer, RendererOptions};

/// Where audio comes from. Loopback must be polled on this thread because
/// cpal streams are `!Send`; the synth runs entirely on its own thread.
enum AudioInput {
    Loopback(Box<LoopbackCapture>),
    Synth,
}

impl AudioInput {
    fn poll(&mut self) {
        if let AudioInput::Loopback(c) = self {
            c.poll();
        }
    }
}

/// `--tour`: visit every preset, screenshot each, exit.
struct Tour {
    dwell: f32,
    index: usize,
    since: Instant,
}

struct App {
    args: Args,
    window: Option<Arc<Window>>,
    ctx: Option<RenderContext>,
    renderer: Option<Renderer>,
    features: Output<AudioFeatures>,
    audio: AudioInput,
    fullscreen: bool,
    reconfigure: bool,
    started: Instant,
    screenshot_requested: bool,
    screenshot_at: Option<f32>,
    tour: Option<Tour>,
    title: String,
    exit_requested: bool,
}

impl App {
    fn toggle_fullscreen(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        window.set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(None)));
    }

    fn render(&mut self) {
        self.audio.poll();

        let (Some(ctx), Some(renderer), Some(window)) =
            (self.ctx.as_mut(), self.renderer.as_mut(), self.window.as_ref())
        else {
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
                // Minimized windows report Occluded immediately; don't spin a core.
                std::thread::sleep(std::time::Duration::from_millis(16));
                window.request_redraw();
                return;
            }
            other => {
                error!(?other, "surface acquire failed");
                window.request_redraw();
                return;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        renderer.render(ctx, &view, &features);
        ctx.queue.present(frame);
        if std::mem::take(&mut self.reconfigure) {
            ctx.surface.configure(&ctx.device, &ctx.config);
        }

        // Screenshots re-run the display passes offscreen for the frame just drawn.
        let elapsed = self.started.elapsed().as_secs_f32();
        if self.screenshot_at.is_some_and(|t| elapsed >= t) {
            self.screenshot_at = None;
            self.screenshot_requested = true;
        }
        if std::mem::take(&mut self.screenshot_requested) {
            let path = render::screenshot::default_path(renderer.library().current_name());
            match renderer.screenshot(ctx, &path) {
                Ok(()) => info!(path = %path.display(), "screenshot saved"),
                Err(e) => warn!("screenshot failed: {e:#}"),
            }
        }

        if let Some(tour) = self.tour.as_mut()
            && tour.since.elapsed().as_secs_f32() >= tour.dwell
        {
            let lib = renderer.library();
            let name = lib.current_name().to_owned();
            let path = PathBuf::from("screenshots/tour")
                .join(format!("{:02}-{}.png", tour.index, name.to_ascii_lowercase().replace(' ', "-")));
            match renderer.screenshot(ctx, &path) {
                Ok(()) => info!(preset = name, path = %path.display(), "tour screenshot"),
                Err(e) => warn!("tour screenshot failed: {e:#}"),
            }
            tour.index += 1;
            if tour.index >= renderer.library().len() {
                self.exit_requested = true;
            } else {
                renderer.library_mut().cut_to(tour.index);
                tour.since = Instant::now();
            }
        }

        let title = format!("Seoul — {}", renderer.library().current_name());
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }

        window.request_redraw();
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, kc: KeyCode) {
        let lib = self.renderer.as_mut().map(|r| r.library_mut());
        match (kc, lib) {
            (KeyCode::Escape, _) => event_loop.exit(),
            (KeyCode::F11, _) => self.toggle_fullscreen(),
            (KeyCode::KeyP, _) => self.screenshot_requested = true,
            (KeyCode::Space, Some(lib)) => {
                lib.next();
                info!(preset = lib.current_name(), "next");
            }
            (KeyCode::Backspace, Some(lib)) => {
                lib.prev();
                info!(preset = lib.current_name(), "prev");
            }
            (KeyCode::KeyR, Some(lib)) => {
                lib.random();
                info!(preset = lib.current_name(), "random");
            }
            (KeyCode::KeyA, Some(lib)) => {
                let on = lib.toggle_auto_advance();
                info!(auto_advance = on, "auto-advance");
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let size: Size = match self.args.size {
            Some((w, h)) => PhysicalSize::new(w, h).into(),
            None => LogicalSize::new(1280, 720).into(),
        };
        let attrs = Window::default_attributes().with_title("Seoul").with_inner_size(size);
        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let ctx = pollster::block_on(RenderContext::new(window.clone())).expect("failed to create render context");
        let opts = RendererOptions {
            render_scale: self.args.render_scale.unwrap_or(1.0),
            post_defaults: PostParams::default(),
        };
        let mut renderer = Renderer::new(&ctx, &PathBuf::from("presets"), opts).expect("failed to load presets");

        if let Some(name) = &self.args.preset {
            match renderer.library().find(name) {
                Some(i) => renderer.library_mut().cut_to(i),
                None => warn!(preset = name, "no such preset; starting on default"),
            }
        }
        if let Some(dwell) = self.args.tour {
            renderer.library_mut().cut_to(0);
            self.tour = Some(Tour {
                dwell,
                index: 0,
                since: Instant::now(),
            });
        }

        window.request_redraw();
        self.window = Some(window);
        self.ctx = Some(ctx);
        self.renderer = Some(renderer);
        if self.args.fullscreen {
            self.toggle_fullscreen();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(ctx) = self.ctx.as_mut() {
                    ctx.resize(size);
                }
                if let (Some(ctx), Some(renderer)) = (self.ctx.as_ref(), self.renderer.as_mut()) {
                    renderer.resize(ctx, size);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(kc),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => self.on_key(event_loop, kc),
            WindowEvent::RedrawRequested => {
                self.render();
                if self.exit_requested {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    let Some(args) = Args::parse()? else {
        return Ok(());
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "seoul=info,wgpu_core=warn,wgpu_hal=warn".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("seoul starting");

    let (features, source_tx) = audio::analysis::spawn_analysis();
    let audio = if args.synth {
        audio::synth::spawn_synth(source_tx);
        AudioInput::Synth
    } else {
        AudioInput::Loopback(Box::new(LoopbackCapture::new(source_tx)))
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        screenshot_at: args.screenshot_at,
        args,
        window: None,
        ctx: None,
        renderer: None,
        features,
        audio,
        fullscreen: false,
        reconfigure: false,
        started: Instant::now(),
        screenshot_requested: false,
        tour: None,
        title: String::new(),
        exit_requested: false,
    };

    event_loop.run_app(&mut app)?;
    Ok(())
}
