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
mod config;
mod preset;
mod render;

use crate::audio::AudioFeatures;
use crate::audio::capture::LoopbackCapture;
use crate::cli::Args;
use crate::config::{Config, ConfigWatch};
use crate::preset::curation::{Curation, STATE_PATH};
use crate::preset::library::LibrarySettings;
use crate::preset::transition::TransitionStyle;
use crate::render::{RenderContext, Renderer, RendererOptions};

/// Merge config with command-line overrides into library behavior.
fn library_settings(cfg: &Config, args: &Args) -> LibrarySettings {
    let mut s = LibrarySettings {
        post_defaults: cfg.post.resolve(),
        auto_advance: cfg.auto.enabled,
        auto_min: cfg.auto.min_interval,
        auto_max: cfg.auto.max_interval.max(cfg.auto.min_interval),
        transition_style: cfg.transition.style().unwrap_or(None),
        transition_duration: cfg.transition.duration,
    };
    if let Some(secs) = args.auto {
        s.auto_advance = true;
        s.auto_min = secs;
        s.auto_max = secs * 1.5;
    }
    if let Some(style) = &args.transition {
        match style.as_str() {
            "random" => s.transition_style = None,
            other => match TransitionStyle::parse(other) {
                Some(t) => s.transition_style = Some(t),
                None => warn!(style = other, "unknown --transition style; using config"),
            },
        }
    }
    s
}

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
    /// Screenshots per preset, evenly spaced through the dwell.
    shots: u32,
    taken: u32,
    index: usize,
    since: Instant,
    /// Only visit presets whose name contains this (case-insensitive).
    filter: Option<String>,
}

impl Tour {
    /// Next preset index at or after `from` that passes the filter.
    fn next_match(&self, lib: &preset::PresetLibrary, from: usize) -> Option<usize> {
        (from..lib.len()).find(|&i| {
            self.filter
                .as_ref()
                .is_none_or(|f| lib.name_at(i).to_ascii_lowercase().contains(&f.to_ascii_lowercase()))
        })
    }
}

struct App {
    args: Args,
    config: Config,
    config_watch: Option<ConfigWatch>,
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
    fn render(&mut self) {
        self.audio.poll();
        if let Some(cfg) = self.config_watch.as_mut().and_then(|w| w.poll()) {
            if let Some(r) = self.renderer.as_mut() {
                let mut settings = library_settings(&cfg, &self.args);
                // Keep a live A-key toggle unless the file itself changed it.
                if cfg.auto.enabled == self.config.auto.enabled {
                    settings.auto_advance = r.library().auto_advance();
                }
                r.library_mut().set_settings(settings);
            }
            self.config = cfg;
        }

        let (Some(ctx), Some(renderer), Some(window)) =
            (self.ctx.as_mut(), self.renderer.as_mut(), self.window.as_ref())
        else {
            return;
        };

        let features = *self.features.read();
        let status = match &self.audio {
            AudioInput::Synth => "synth test track".to_owned(),
            AudioInput::Loopback(c) => c.device_name().map_or_else(|| "no audio — retrying".to_owned(), str::to_owned),
        };
        renderer.hud_mut().set_audio_status(&status);

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
            && tour.since.elapsed().as_secs_f32() >= tour.dwell * (tour.taken + 1) as f32 / tour.shots as f32
        {
            let lib = renderer.library();
            let name = lib.current_name().to_owned();
            let slug = name.to_ascii_lowercase().replace(' ', "-");
            let file = if tour.shots > 1 {
                format!("{:02}-{slug}-{}.png", tour.index, tour.taken + 1)
            } else {
                format!("{:02}-{slug}.png", tour.index)
            };
            let path = PathBuf::from("screenshots/tour").join(file);
            match renderer.screenshot(ctx, &path) {
                Ok(()) => info!(preset = name, path = %path.display(), "tour screenshot"),
                Err(e) => warn!("tour screenshot failed: {e:#}"),
            }
            tour.taken += 1;
            if tour.taken >= tour.shots {
                match tour.next_match(renderer.library(), tour.index + 1) {
                    None => self.exit_requested = true,
                    Some(i) => {
                        tour.index = i;
                        tour.taken = 0;
                        renderer.library_mut().cut_to(i);
                        tour.since = Instant::now();
                    }
                }
            }
        }

        let title = format!("Seoul — {}", renderer.library().current_name());
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }

        window.request_redraw();
    }

    fn toggle_fullscreen_on(&mut self, monitor: Option<usize>, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        let target = monitor.and_then(|i| event_loop.available_monitors().nth(i));
        window.set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(target)));
        // Nothing to click in a visualizer; keep the pointer out of the art.
        window.set_cursor_visible(!self.fullscreen);
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, kc: KeyCode) {
        match kc {
            KeyCode::Escape => return event_loop.exit(),
            KeyCode::F11 => return self.toggle_fullscreen_on(self.config.monitor, event_loop),
            KeyCode::KeyP => {
                self.screenshot_requested = true;
                return;
            }
            _ => {}
        }
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let notice: Option<String> = match kc {
            KeyCode::Space => {
                r.library_mut().next();
                None
            }
            KeyCode::Backspace => {
                r.library_mut().prev();
                None
            }
            KeyCode::KeyR => {
                r.library_mut().random();
                None
            }
            KeyCode::KeyA => {
                let on = r.library_mut().toggle_auto_advance();
                Some(format!("Auto-advance {}", if on { "on" } else { "off" }))
            }
            KeyCode::KeyF => {
                let on = r.library_mut().toggle_favorite();
                let name = r.library().current_name();
                Some(if on {
                    format!("★  {name} added to favorites")
                } else {
                    format!("{name} removed from favorites")
                })
            }
            KeyCode::KeyX => {
                let name = r.library().current_name().to_owned();
                let hidden = r.library_mut().toggle_hidden();
                Some(if hidden { format!("{name} hidden") } else { format!("{name} unhidden") })
            }
            KeyCode::KeyL => {
                let on = r.library_mut().toggle_locked();
                Some(if on { "Locked — auto-advance paused".into() } else { "Unlocked".into() })
            }
            KeyCode::KeyH => {
                let hud = r.hud_mut();
                hud.show_help = !hud.show_help;
                None
            }
            KeyCode::F1 => {
                let hud = r.hud_mut();
                hud.show_stats = !hud.show_stats;
                None
            }
            KeyCode::Digit1
            | KeyCode::Digit2
            | KeyCode::Digit3
            | KeyCode::Digit4
            | KeyCode::Digit5
            | KeyCode::Digit6
            | KeyCode::Digit7
            | KeyCode::Digit8
            | KeyCode::Digit9 => {
                let n = kc as usize - KeyCode::Digit1 as usize;
                if r.library_mut().jump_favorite(n) {
                    None
                } else {
                    Some(format!("No favorite #{} yet — press F to add one", n + 1))
                }
            }
            _ => None,
        };
        info!(preset = r.library().current_name(), key = ?kc, "key");
        if let Some(text) = notice {
            r.hud_mut().notice(text);
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
            render_scale: self.args.render_scale.unwrap_or(self.config.render_scale),
            library: library_settings(&self.config, &self.args),
            curation: Curation::load(&PathBuf::from(STATE_PATH)),
            ui_scale: window.scale_factor(),
        };
        let mut renderer = Renderer::new(&ctx, &PathBuf::from("presets"), opts).expect("failed to load presets");
        {
            let hud = renderer.hud_mut();
            hud.enabled = self.config.hud.enabled;
            hud.show_stats = self.config.hud.stats;
            hud.toast_seconds = self.config.hud.toast_seconds;
            if self.args.tour.is_some() {
                // Clean frames for preset review.
                hud.enabled = false;
                hud.skip_intro();
            }
        }

        // In a tour, --preset is a name filter, handled below.
        let start = if self.args.tour.is_some() {
            None
        } else {
            self.args.preset.as_ref().or(self.config.start_preset.as_ref())
        };
        if let Some(name) = start {
            match renderer.library().find(name) {
                Some(i) => renderer.library_mut().cut_to(i),
                None => warn!(preset = name, "no such preset; starting on default"),
            }
        }
        if let Some(dwell) = self.args.tour {
            let mut tour = Tour {
                dwell,
                shots: self.args.tour_shots.unwrap_or(1).max(1),
                taken: 0,
                index: 0,
                since: Instant::now(),
                filter: self.args.preset.clone(),
            };
            match tour.next_match(renderer.library(), 0) {
                Some(i) => {
                    tour.index = i;
                    renderer.library_mut().cut_to(i);
                    self.tour = Some(tour);
                }
                None => {
                    warn!("--tour: no preset matches the --preset filter");
                    self.exit_requested = true;
                }
            }
        }

        window.request_redraw();
        self.window = Some(window);
        self.ctx = Some(ctx);
        self.renderer = Some(renderer);
        if self.args.fullscreen || self.config.fullscreen {
            self.toggle_fullscreen_on(self.config.monitor, event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(r) = self.renderer.as_mut() {
                    r.hud_mut().set_scale(scale_factor);
                }
            }
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

    let config_path = PathBuf::from(args.config.as_deref().unwrap_or(config::DEFAULT_PATH));
    let config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("{e:#} — using defaults");
            Config::default()
        }
    };
    let config_watch = match ConfigWatch::new(&config_path) {
        Ok(w) => Some(w),
        Err(e) => {
            warn!("config hot-reload disabled: {e:#}");
            None
        }
    };

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
        config,
        config_watch,
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
