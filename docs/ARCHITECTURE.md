# Seoul — Implementation Guide

How Seoul turns system audio into feedback-driven visuals: threads, the
analysis math, the per-frame GPU pass graph, the preset system, and the
invariants that keep it all consistent. For writing presets, see
[`PRESETS.md`](PRESETS.md).

## Table of contents

- [Thread topology](#thread-topology)
- [Audio capture](#audio-capture)
- [Feature extraction](#feature-extraction)
- [`AudioFeatures`: CPU snapshot and GPU layout](#audiofeatures-cpu-snapshot-and-gpu-layout)
- [Timing model](#timing-model)
- [Render pass graph](#render-pass-graph)
- [Transitions and masks](#transitions-and-masks)
- [Preset system](#preset-system)
- [Hot reload](#hot-reload)
- [Curation, auto-advance, config](#curation-auto-advance-config)
- [HUD](#hud)
- [Verification tooling](#verification-tooling)
- [Invariants checklist](#invariants-checklist)

---

## Thread topology

```
 WASAPI callback ──ring (f32 mono)──▶ analysis thread ──triple buffer──▶ render thread
 (cpal, realtime)                     (Analyzer, ~5.8 ms hop)            (winit loop, wgpu)
        ▲                                     ▲
        └──── LoopbackCapture (main thread) ──┘  sends a new ring on (re)connect
```

- **Audio callback** (`audio/capture.rs`): downmixes any sample format
  (F32/F64/I16/I24/I32/U16) to mono f32 and pushes into a `ringbuf::HeapRb`.
  Drops samples rather than blocking if analysis falls behind.
- **Analysis thread** (`audio/analysis.rs`): drains the ring, runs the
  `Analyzer`, publishes `AudioFeatures` into a `triple_buffer`. It receives
  new `AudioSource`s (ring + sample rate) over an mpsc channel whenever
  capture reconnects, rebuilding the analyzer if the rate changes.
- **Render thread** (`main.rs`, `render/`): owns the window, polls capture,
  reads the latest features, renders. `--synth` replaces loopback with a
  generator thread (`audio/synth.rs`) feeding the same channel.

## Audio capture

cpal implements WASAPI loopback implicitly: `build_input_stream` on a
*render* device, with the config from `default_output_config()`, makes cpal
set `AUDCLNT_STREAMFLAGS_LOOPBACK`. This is still true in cpal 0.18.

`LoopbackCapture` lives on the main thread (cpal streams are `!Send`) and
is polled every frame:

- The stream's error callback sets an atomic flag for anything but `Xrun`.
  cpal 0.18 reports `StreamInvalidated` / `DeviceNotAvailable` when the
  default output device changes; the flag triggers a reconnect after 250 ms.
- Every 2 s it also compares the default device's id with the active one
  (belt and braces).
- With no device, the app runs on silence and retries every 2 s.

## Feature extraction

`Analyzer` is a pure DSP core: push samples in, read `features()` out. It
steps on a fixed **256-sample hop of the sample clock**, so every time
constant is in seconds of audio and tests are deterministic.

Per hop, over the last 2048 samples:

| Feature | Method |
|---|---|
| FFT | Hann window, real FFT, magnitudes normalized so a full-scale sine ≈ 1.0 |
| `bass` `mid` `treble` | band *energy* (√Σm², not mean, so wide bands aren't diluted) in 40–250 Hz / 250 Hz–4 kHz / 4–16 kHz; **per-band AGC**: divide by a reference that tracks peaks and relaxes over ~3 s toward a −50 dBFS floor; then a follower (12 ms attack, 150 ms release) |
| `*_att` | ~1 s follower of each band (MilkDrop semantics) |
| `volume` | RMS with the same AGC + follower |
| `spectrum[64]` | log-spaced 40 Hz–16 kHz, per-bin power in dB + 3 dB/octave tilt, mapped to 0..1 below a reference that tracks the loudest bin (6 dB/s release, −35 dB floor, 48 dB range); fast rise, slow fall |
| `beat` | spectral flux of log-compressed magnitudes (kick band 30–180 Hz + ½ broadband), adaptive threshold mean + 1.5σ over ~1 s, one-hop-late peak picking, 180 ms refractory; envelope jumps to 1 and decays with τ = 100 ms |
| `bpm`, `bpm_confidence` | inter-onset-interval histogram over the last 10 s (pairs 0.25–2 s apart, folded into 80–160 BPM, Gaussian-smeared, parabolic peak); confidence = mass near the peak |
| `beat_phase` | oscillator at `bpm`, nudged toward 0 when a detected beat lands near it |
| `waveform[512]` | trigger-aligned to the steepest rising zero crossing, auto-gained, 24-sample sin² taper at both ends (so circular plots close), light temporal smoothing |

**Silence.** WASAPI loopback delivers *no packets* while nothing plays. If
nothing arrives for 40 ms, the thread feeds real-time zeros, so every
envelope decays exactly as it would on digital silence; nothing latches.

Tests (`audio::analysis::tests`) cover silence, sub-floor noise, kick
detection at 100/120/140 BPM, envelope decay, waveform stability, taper,
and per-band balance.

## `AudioFeatures`: CPU snapshot and GPU layout

`AudioFeatures` (`audio/features.rs`) is `#[repr(C)] Pod`: 20 scalars
(including `resolution: vec2` on a 16-byte boundary), then
`spectrum[64]` and `waveform[512]`. It is uploaded verbatim to a storage
buffer bound by every composite, warp and particle pipeline. The WGSL
mirror lives in **`shaders/common.wgsl`** and must match field for field; a
`const` size assertion in Rust guards drift.

## Timing model

- The **render thread owns the clock**: it stamps `time`, `dt`, `frame`,
  `aspect`, `resolution` into its copy of the features each frame. Analysis
  can stall; visuals never freeze.
- Presets are authored **"per frame at 60 Hz"**. `WarpParams::to_uniforms`
  rescales by `k = dt·60`: multiplicative terms (`decay`, `zoom`, `sx`,
  `sy`) are raised to `k`, additive ones (`rotation`, `dx`, `dy`, wobble,
  `sharpen`) multiplied by `k`, `blur` becomes `1 − (1 − b)^k`, `hue_shift`
  is per second. The composite mask carries `k` too, so per-frame additive
  energy scales with frame time. Result: the same trails and brightness at
  60 and 144 Hz (unit-tested).

## Render pass graph

Two ping-pong `Rgba16Float` feedback textures (`render/feedback.rs`)
follow the window size × `render_scale`, capped at 4096. On resize the
current image is resampled into the new pair so nothing flashes.

```
 read feedback ─▶ 1 WARP ─▶ write feedback ─▶ 1b PARTICLES ─▶ 2 COMPOSITE ─▶ 3 BLOOM ─▶ 3 POST ─▶ swapchain ─▶ 4 HUD
                  (masked,                     (additive,       (masked,        (mip       (tonemap,
                  per preset)                  if any)          per preset)     chain)     grade)
```

1. **Warp** (`render/warp.rs`, `shaders/warp_prelude.wgsl` +
   `warp_default.wgsl` or a preset's own `fs_warp`): a fullscreen per-pixel
   resample of last frame. The built-in transform zooms/stretches/rotates
   about `(cx, cy)` **in aspect-corrected space** (so rotation never
   shears), translates, adds the MilkDrop sine wobble, then optional
   blur/sharpen and hue rotation, × `decay`. Edge policy: mirror
   (default, so zoom-out never smears border streaks), fade, or clamp. Each
   active preset gets its own draw with its own uniforms and pipeline.
2. **Particles** (`render/particles.rs`, `shaders/particles_*.wgsl`): when
   any active preset has `[particles]`, a compute pass updates up to
   131 072 particles (respawn dead ones in a spawn shape; curl-noise flow,
   bass push, gravity, drag, beat bursts), then instanced soft sprites are
   added into the feedback, where the next warp turns them into trails.
   One population persists across presets; parameters crossfade.
3. **Composite** (`preset/shader.rs`): the preset's `fs_composite`, drawn
   additively on top (one draw per active preset).
4. **Bloom** (`render/post.rs`, `shaders/bloom.wgsl`): COD:AW-style chain
   over up to 6 mips at half resolution. A 13-tap downsample with Karis
   averaging and a soft-knee threshold at mip 0, then 13-tap downsamples,
   then 9-tap tent upsamples blended additively back up.
5. **Post** (`shaders/post.wgsl`): display-time mirror/kaleidoscope fold
   and MilkDrop video echo (neither feeds back), optional LED-panel
   resample or radial chromatic aberration, exposure, contrast and
   saturation in linear light, **Khronos PBR Neutral** tonemap (identity
   below ~0.76, so authored colors survive), then vignette, luma-weighted
   film grain and ±1 LSB triangular dither **in the encoded domain** (in
   linear light the sRGB curve would amplify them ~13× near black). The
   result is linearized for the sRGB swapchain.
6. **HUD** — see [HUD](#hud).

Then `feedback.swap()`.

## Transitions and masks

A transition is `PresetState::Transitioning { from, to, progress, style,
seed }` (`preset/transition.rs`). Progress is linear and eased with
smootherstep at use. Retargeting mid-transition never pops the dominant
layer: returning to `from` reverses in place; otherwise whichever preset
carries more weight keeps its exact weight and the minor layer is swapped
for the new target (the symmetric easing makes this exact; tested).

Styles (crossfade, dissolve, radial, clock, zoom) are **per-pixel** without
any preset knowing about them (`render/mask.rs`, `shaders/mask.wgsl`):

- The feedback texture's alpha channel is otherwise unused. Before each
  preset's warp and composite draw, an alpha-only fullscreen pass writes
  that preset's weight at every pixel.
- Warp and composite pipelines blend `dst.rgb += src.rgb × dst.alpha` with a
  color-only write mask, so the weight sticks until the next mask pass.
- Stable frames use the same path with a uniform weight (and the composite
  mask carries the frame-time factor `k`).
- Zoom style additionally scales the outgoing preset's zoom up as it fades.

Because warps are weighted too, a transition blends the two presets'
feedback *dynamics*, not just their overlays: inside a dissolve, each
region keeps evolving under its own preset's motion.

## Preset system

`preset/preset.rs` parses TOML into a `PresetSpec`: expressions parsed once
into `Expr` trees (`preset/expr.rs`, recursive descent, WGSL-named
functions), optional mapping fields defaulted, `[post]` numbers or
expressions, `[particles]`, palette, edge mode, shader paths relative to
the TOML. Unknown keys are rejected everywhere.

`preset/library.rs` builds each spec into a `Preset` (composite pipeline,
optional custom warp pipeline, palette uniform) and produces a per-frame
`FramePlan`: per-preset warp steps, composite draws, folded post params,
particle plan, and the transition mask.

Shader compilation (`preset/shader.rs::compile_wgsl`) prepends
`common.wgsl` + the relevant prelude and wraps **both** shader-module and
render-pipeline creation in wgpu validation error scopes, so any authoring
mistake (syntax, bad entry point, wrong return type) is an `Err`, never a
device panic. naga's `wgsl:LINE:COL` locations and gutter numbers are
remapped from the combined source back to the author's file.

Bind group layouts are shared: `audio` (storage, all stages) and `palette`
(uniform, all stages) are created once in `Renderer::new` and reused by the
composite (`[audio, palette]`), warp (`[prev+sampler, warp uniforms,
audio, palette]`) and particle pipelines.

## Hot reload

`preset/watcher.rs` (notify, recursive) forwards create/modify/remove events
for `.toml`/`.wgsl`. `PresetLibrary::poll_reloads` debounces per path
(120 ms), then decides by what exists on disk:

- TOML exists → rebuild that preset, or append it if new.
- TOML gone → remove the preset if it isn't on screen (indices in the state
  machine shift; tested).
- WGSL changed → rebuild every preset using it; if none does, scan for
  unloaded TOMLs (a preset whose shader arrived after its TOML).
- Any failure keeps the last good version running and is surfaced in the
  HUD's error panel until the next success.

`seoul.toml` has its own watcher (`config.rs::ConfigWatch`).

## Curation, auto-advance, config

- `preset/curation.rs`: favorites and hidden sets persisted by name to
  `seoul-state.toml`; a shuffle bag that plays every visible preset once
  per cycle (favorites twice) and never repeats the current one.
- Auto-advance (`PresetLibrary::tick`): after `min_interval`, change on a
  beat, and only on a 4-beat boundary when tempo confidence ≥ 0.4; always
  change at `max_interval` (so silence still advances). Lock (L) pauses it.
- `config.rs`: `seoul.toml` sections `[auto]`, `[transition]`, `[post]`
  (global look defaults), `[hud]`, plus start preset, fullscreen/monitor,
  render scale. CLI flags override.

## HUD

`render/hud.rs` draws after tonemapping, so it never blooms. Text is
glyphon/cosmic-text over fonts loaded straight from `C:\Windows\Fonts`
(Bahnschrift, Malgun Gothic for Hangul, Consolas), with a full system scan
as a fallback. Text buffers re-shape only when their content changes;
panels and meters are instanced SDF rounded rects (`shaders/hud.wgsl`)
with premultiplied blending. Elements: startup 서울 wordmark, preset toasts
(palette-colored accent, drop shadow), key notices, help (H), stats (F1:
FPS, tempo, beat phase, band meters with ~1 s average ticks, spectrum
strip, audio device), and the preset error panel. `P` screenshots include
the HUD (the post and HUD passes are re-run offscreen for the last frame).

## Verification tooling

- `--synth`: deterministic 124 BPM test track (kick, clap, hats, ducked
  bass, pad, arpeggio, kick-less breakdown every eighth bar).
- `--tour SECS [--tour-shots N] [--preset filter]`: hard-cut through
  presets, screenshot each into `screenshots/tour/`, exit. The HUD is
  disabled for clean frames.
- `--screenshot-at SECS`, `P`: PNG of the current frame.

## Invariants checklist

- `AudioFeatures` (Rust) ⇔ `shaders/common.wgsl` field for field.
- `WarpUniforms` (80 B), `PostUniforms` (80 B), `ParticleUniforms` (96 B),
  mask uniforms (32 B): Rust structs ⇔ WGSL structs, guarded by size asserts.
- Everything drawn into the feedback that should respect transitions
  (warp, composite) uses `MASKED_BLEND` + `ColorWrites::COLOR`; nothing
  else may write feedback alpha except the mask pass.
- Per-frame quantities authored at 60 Hz must go through `k = dt·60`.
- Anything new compiled from user files goes inside an error scope.
