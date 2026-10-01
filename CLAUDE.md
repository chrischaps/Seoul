# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`seoul` is a MilkDrop-inspired real-time audio visualizer in Rust. It captures desktop audio via WASAPI loopback, extracts features (FFT bands, beat, waveform), and renders feedback-driven shader visuals with wgpu. Presets are hot-reloadable TOML + WGSL pairs.

## Commands

```bash
cargo run --release        # primary — debug builds are too slow for 60 fps
cargo build --release
cargo test                 # unit tests live alongside the code (analysis, expr, preset, warp, shader, …)
cargo test <name>          # run a single test
RUST_LOG=seoul=debug cargo run --release   # per-module log level via EnvFilter

# Visual verification (no music needed):
cargo run --release -- --synth --tour 5 --size 1280x720   # screenshot every preset → screenshots/tour/, then exit
cargo run --release -- --synth --preset Vortex --screenshot-at 5
cargo run --release -- --help
```

`--synth` swaps loopback for a deterministic built-in 124 BPM test track (`src/audio/synth.rs`). Screenshots re-run the display passes offscreen and are viewable with the Read tool — use them to check any visual change before claiming it works.

Windows-only: audio capture uses the WASAPI host explicitly. There is no cross-platform audio path.

### Runtime keys
Space = next preset · Backspace = prev · R = random · A = toggle auto-advance · P = screenshot · F11 = fullscreen · Esc = quit.

## Architecture

Three long-lived threads exchange data through lock-free structures:

1. **Audio callback (cpal / WASAPI loopback)** — `src/audio/capture.rs`. Downmixes to mono f32 and pushes into a `ringbuf::HeapRb`. `LoopbackCapture` (main thread, polled per frame) reopens the stream on errors or default-device changes and sends each new ring to the analysis thread.
2. **Analysis thread** — `src/audio/analysis.rs`. A pure `Analyzer` steps on a 256-sample hop of the *sample* clock: 2048-pt FFT, per-band AGC'd bass/mid/treble (+ ~1 s `*_att`), dB spectrum, spectral-flux beat detection, IOI-histogram tempo + phase oscillator, trigger-aligned waveform. When loopback goes quiet (WASAPI sends no packets) the thread feeds real-time zeros so everything decays naturally. Publishes into a `triple_buffer`.
3. **Render thread (winit event loop)** — `src/main.rs` + `src/render/`. Reads the latest `AudioFeatures`, **stamps the clock fields** (`time`, `dt`, `frame`), uploads to GPU, renders. Time lives here, not in analysis, so visuals never freeze.

`AudioFeatures` (`src/audio/features.rs`) is `#[repr(C)] Pod` and is **both the CPU-side feature snapshot and the exact GPU storage-buffer layout** bound at `@group(0) @binding(0)` in every composite shader. Its layout must stay in sync with `shaders/composite_prelude.wgsl`.

### Render pipeline (`src/render/renderer.rs::render`)

Two ping-pong `Rgba16Float` feedback textures (`render/feedback.rs`) follow the window size × `--render-scale`; on resize the current image is resampled into the new pair. Each frame:

1. **Warp pass** (`render/warp.rs` + `shaders/warp.wgsl`) — a fullscreen per-pixel resample of last frame through zoom/rotate/stretch about `(cx, cy)` in aspect-correct space, translate, sinusoidal wobble, optional blur/sharpen/hue drift, × `decay`. Edge policy is mirror (default), fade or clamp.
2. **Composite pass** (`preset/shader.rs`) — one or two fullscreen draws that **additively** blend the preset's `fs_composite` output on top of the warped feedback (`src = Constant, dst = One`). The blend constant carries transition intensity × frame-time normalization.
3. **Post** (`render/post.rs` + `shaders/bloom.wgsl`, `shaders/post.wgsl`) — mip-chain bloom, then exposure/saturation → Khronos PBR Neutral tonemap → vignette, grain, dither, optional chromatic aberration and LED-panel mask, into the sRGB swapchain.

Textures then swap (`feedback.swap()`), so next frame's warp reads what this frame wrote.

**Frame-rate independence:** presets are authored "per frame at 60 Hz". `WarpParams::to_uniforms` raises multiplicative terms (decay, zoom, stretch) to `dt·60` and scales additive ones, and the composite blend constant is scaled by `dt·60`, so trails and brightness are identical at 60 and 144 Hz.

### Preset system (`src/preset/`)

A preset is a `.toml` + `.wgsl` pair in `presets/`. Authors write **only** the fragment function `fs_composite` — `shaders/composite_prelude.wgsl` is prepended at compile time and provides: `AudioFeatures`/`Palette` bindings, the `Varying` struct, `vs_fullscreen`, and `waveform_at` / `spectrum_at` helpers. Do not redeclare these in preset shaders.

Each TOML declares four required expressions under `[mapping]` — `zoom`, `rotation`, `warp_amount`, `decay` — plus optional `cx cy dx dy sx sy warp_scale warp_speed hue_shift blur sharpen`, written in a tiny mini-language parsed by `preset/expr.rs`. Optional top-level `edge = "mirror"|"fade"|"clamp"` and a `[post]` table overriding look defaults (`exposure bloom bloom_threshold chroma vignette grain led_mask led_pitch saturation contrast`). Unknown keys are rejected. Available variables: `bass mid treble bass_att mid_att treble_att volume beat time dt frame bpm beat_phase beat_count aspect`. Available functions (WGSL semantics): `sin cos tan abs sqrt exp log floor fract sign pow min max atan2 step clamp mix smoothstep`. These expressions are parsed once at load and evaluated every frame to drive the warp/decay of pass 1.

Preset compilation goes through `preset/shader.rs`: wgpu validation error scopes wrap both `create_shader_module` **and** `create_render_pipeline`, so a bad preset returns `Err` instead of panicking the device — this is what makes hot-reload safe. naga error locations are remapped from prelude+body to the author's file/line. Presets use `centered(uv)` / `polar(uv)` from the prelude for aspect-correct shapes.

`preset/library.rs` owns all compiled presets and a `PresetState` (`preset/transition.rs`) that is either `Stable` or `Transitioning`. `frame_plan()` produces a `FramePlan` with the evaluated `WarpParams`, `PostParams`, and 1–2 `CompositeDraw`s for the current frame. `poll_reloads()` debounces events from `preset/watcher.rs` (notify crate, 120 ms) — editing a `.toml`/`.wgsl` rebuilds that preset live, new presets are added and deleted ones removed; on failure the last good version keeps running.

For an in-depth walkthrough (feature extraction math, pass-by-pass GPU dataflow, preset system internals, WGSL/Rust layout constraints), see `docs/ARCHITECTURE.md`.

## Audio capture gotcha (WASAPI loopback)

cpal's WASAPI backend implements loopback via a specific pattern: call `build_input_stream` **on the default output device**, using the config returned by `default_output_config()` (not `default_input_config()`). The backend detects this combination and sets `AUDCLNT_STREAMFLAGS_LOOPBACK`. If capture ever silently stops working, re-check `src/audio/capture.rs::open_loopback` — the device/config pairing is the load-bearing part. (Still true as of cpal 0.18; it also reports `StreamInvalidated`/`DeviceNotAvailable` on default-device changes, which `LoopbackCapture` uses to reconnect.)
