# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`seoul` is a MilkDrop-inspired real-time audio visualizer in Rust. It captures desktop audio via WASAPI loopback, extracts features (FFT bands, beat, waveform), and renders feedback-driven shader visuals with wgpu. Presets are hot-reloadable TOML + WGSL pairs.

## Commands

```bash
cargo run --release        # primary — debug builds are too slow for 60 fps
cargo build --release
cargo test                 # unit tests live alongside the code (expr.rs, preset.rs, transition.rs)
cargo test <name>          # run a single test
RUST_LOG=seoul=debug cargo run --release   # per-module log level via EnvFilter
```

Windows-only: audio capture uses the WASAPI host explicitly. There is no cross-platform audio path.

### Runtime keys
Space = next preset · Backspace = prev · R = random · A = toggle auto-advance · F11 = fullscreen · Esc = quit.

## Architecture

Three long-lived threads exchange data through lock-free structures:

1. **Audio callback (cpal / WASAPI loopback)** — `src/audio/capture.rs`. Downmixes to mono f32 and pushes into a `ringbuf::HeapRb`.
2. **Analysis thread** — `src/audio/analysis.rs`. Pops from the ring, runs a 2048-pt Hann-windowed real FFT, computes log-spaced spectrum, bass/mid/treble bands, RMS volume, and beat detection (history-ratio on smoothed bass). Writes an `AudioFeatures` snapshot each frame into a `triple_buffer`.
3. **Render thread (winit event loop)** — `src/main.rs` + `src/render/`. Reads the latest `AudioFeatures`, uploads to GPU, renders.

`AudioFeatures` (`src/audio/features.rs`) is `#[repr(C)] Pod` and is **both the CPU-side feature snapshot and the exact GPU storage-buffer layout** bound at `@group(0) @binding(0)` in every composite shader. Its layout must stay in sync with `shaders/composite_prelude.wgsl`.

### Render pipeline (`src/render/renderer.rs::render`)

Each frame runs three passes against two ping-pong `Rgba16Float` feedback textures (`render/feedback.rs`, fixed 1280×720):

1. **Warp pass** (`render/warp.rs` + `shaders/warp.wgsl`) — draws a 48×36 grid (`render/mesh.rs`) whose per-vertex UVs were CPU-displaced this frame by `WarpParams { zoom, rotation, warp_amount }`. Samples the *read* feedback texture with linear filtering, multiplied by a `decay` uniform. Writes to the *write* texture. This is the feedback-trail effect.
2. **Composite pass** (`preset/shader.rs`) — one or two fullscreen draws that **additively** blend the preset's `fs_composite` output on top of the warped feedback. Blend mode is `src_factor = Constant, dst_factor = One`, and the per-draw blend constant carries the intensity — during a preset transition the renderer issues two draws (old preset at `1 - progress`, new at `progress`) for a crossfade.
3. **Blit pass** (`render/blit.rs`) — copies the write texture to the swapchain with letterboxing for aspect correction.

Textures then swap (`feedback.swap()`), so next frame's warp reads what this frame wrote.

### Preset system (`src/preset/`)

A preset is a `.toml` + `.wgsl` pair in `presets/`. Authors write **only** the fragment function `fs_composite` — `shaders/composite_prelude.wgsl` is prepended at compile time and provides: `AudioFeatures`/`Palette` bindings, the `Varying` struct, `vs_fullscreen`, and `waveform_at` / `spectrum_at` helpers. Do not redeclare these in preset shaders.

Each TOML declares four expressions under `[mapping]` — `zoom`, `rotation`, `warp_amount`, `decay` — written in a tiny mini-language parsed by `preset/expr.rs`. Available variables: `bass mid treble bass_att mid_att treble_att volume beat time`. Available functions: `sin cos abs sqrt pow min max clamp mix`. These expressions are parsed once at load and evaluated every frame to drive the warp/decay of pass 1.

Preset compilation goes through `preset/shader.rs`: a wgpu validation error scope wraps `create_shader_module` so a bad preset returns `Err` instead of panicking the device — this is what makes hot-reload safe.

`preset/library.rs` owns all compiled presets and a `PresetState` (`preset/transition.rs`) that is either `Stable` or `Transitioning`. `frame_plan()` produces a `FramePlan` with the evaluated `WarpParams`, `decay`, and 1–2 `CompositeDraw`s for the current frame. `poll_reloads()` drains events from `preset/watcher.rs` (notify crate) — editing either the `.toml` or `.wgsl` in `presets/` rebuilds just that preset live.

For an in-depth walkthrough (feature extraction math, pass-by-pass GPU dataflow, preset system internals, WGSL/Rust layout constraints), see `docs/ARCHITECTURE.md`.

## Audio capture gotcha (WASAPI loopback)

cpal's WASAPI backend implements loopback via a specific pattern: call `build_input_stream` **on the default output device**, using the config returned by `default_output_config()` (not `default_input_config()`). The backend detects this combination and sets `AUDCLNT_STREAMFLAGS_LOOPBACK`. If capture ever silently stops working, re-check `src/audio/capture.rs::start_capture` — the device/config pairing is the load-bearing part.
