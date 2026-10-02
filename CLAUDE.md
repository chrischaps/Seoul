# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`seoul` is a MilkDrop-inspired real-time audio visualizer in Rust (wgpu 30, cpal 0.18, winit 0.30). It captures desktop audio via WASAPI loopback, extracts features (AGC'd bands, beats, tempo, spectrum, waveform), and renders feedback-driven HDR shader visuals. Presets are hot-reloadable TOML + WGSL. Windows-only (WASAPI host, HUD fonts from `C:\Windows\Fonts`).

Deeper docs: `docs/ARCHITECTURE.md` (threads, analysis math, pass graph, transitions, invariants) and `docs/PRESETS.md` (authoring guide).

## Commands

```bash
cargo run --release        # primary — debug builds are too slow for 60 fps
run.bat / .\run.ps1        # same, from any cwd; extra args pass through
cargo test                 # unit tests live alongside the code
cargo test <name>
RUST_LOG=seoul=debug cargo run --release

# Visual verification (no music needed) — check every visual change this way:
cargo run --release -- --synth --tour 10 --tour-shots 3 --size 1280x720   # → screenshots/tour/, then exit
cargo run --release -- --synth --tour 10 --preset ink                     # --preset filters a tour by name
cargo run --release -- --synth --preset Vortex --screenshot-at 5
cargo run --release -- --help
```

`--synth` is a deterministic 124 BPM test track (`src/audio/synth.rs`). Screenshots re-run the display passes offscreen and can be viewed with the Read tool. When a script launches the app, send its output to a file, not a pipe: a full pipe blocks the render thread on logging.

Keys: Space/Backspace next/prev · R shuffle · A auto-advance · L lock · F favorite · X hide · 1–9 favorites · H help · F1 stats · P screenshot · F11 fullscreen · Esc quit.

Settings: `seoul.toml` (hot-reloads; CLI overrides). Favorites/hidden: `seoul-state.toml` (gitignored).

## Architecture in brief

- **Threads:** WASAPI callback → ring → analysis thread (`audio/analysis.rs`, pure `Analyzer` on a 256-sample hop; feeds itself real-time zeros when loopback goes quiet) → triple buffer → render thread. `LoopbackCapture` (main thread, polled per frame) reconnects on stream errors and default-device changes.
- **Time is stamped on the render thread** (`time`, `dt`, `frame`, `aspect`, `resolution`), never in analysis.
- **Frame:** warp (per-pixel, per active preset) → particles (compute + sprites, optional) → composite (additive `fs_composite`) → bloom → post (mirror/echo, PBR Neutral tonemap, grade) → HUD. Two ping-pong `Rgba16Float` feedback textures follow the window size.
- **Frame-rate independence:** presets are authored per frame at 60 Hz; `WarpParams::to_uniforms` and the composite mask rescale by `k = dt·60`.
- **Transitions** are per-pixel: `render/mask.rs` writes each preset's weight into the feedback **alpha** channel before its warp/composite draw; those pipelines use `MASKED_BLEND` (`src × DstAlpha`, color-only writes). Presets never handle transitions.
- **Presets:** `preset/preset.rs` (parse; unknown keys rejected), `preset/expr.rs` (mapping expressions), `preset/library.rs` (navigation, curation, auto-advance, `FramePlan`, debounced hot reload with add/remove), `preset/shader.rs` (compile with prelude; **both** shader-module and pipeline creation inside wgpu error scopes so bad presets return `Err`; naga line numbers remapped to the author's file).

## Invariants

- `AudioFeatures` (`src/audio/features.rs`) is both the CPU snapshot and the GPU storage-buffer layout; its WGSL twin is in `shaders/common.wgsl`. Keep them field-for-field identical (a size assert guards drift). The same applies to `WarpUniforms`, `PostUniforms`, `ParticleUniforms` and the mask uniforms versus their WGSL structs.
- Only the mask pass may write feedback alpha; anything else drawn into feedback uses `ColorWrites::COLOR`.
- Anything compiled from user files must go through an error scope.
- Per-frame motion/energy authored at 60 Hz must be scaled by `k = dt·60`.

## Audio capture gotcha (WASAPI loopback)

cpal's WASAPI backend implements loopback via a specific pattern: call `build_input_stream` **on the default output device**, using the config returned by `default_output_config()` (not `default_input_config()`). The backend detects this combination and sets `AUDCLNT_STREAMFLAGS_LOOPBACK`. If capture ever silently stops working, re-check `src/audio/capture.rs::open_loopback`; the device/config pairing is the load-bearing part. This is still true as of cpal 0.18, which also reports `StreamInvalidated`/`DeviceNotAvailable` on default-device changes; `LoopbackCapture` uses those to reconnect. Default configs may now be I32/F64/I24; the generic `push_mono` handles them all.
