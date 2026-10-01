# Seoul — Implementation Guide

An in-depth walkthrough of how `seoul` captures audio, extracts features, and renders feedback-driven visuals. For a quick orientation and commands, see `CLAUDE.md`.

## Table of contents

1. [Thread topology](#thread-topology)
2. [Audio capture (WASAPI loopback)](#audio-capture-wasapi-loopback)
3. [Feature extraction](#feature-extraction)
4. [The `AudioFeatures` shared GPU type](#the-audiofeatures-shared-gpu-type)
5. [Render pipeline](#render-pipeline)
   - [Pass 1 — Warp](#pass-1--warp)
   - [Pass 2 — Composite](#pass-2--composite)
   - [Pass 3 — Blit](#pass-3--blit)
6. [Preset system](#preset-system)
7. [Mapping mini-language](#mapping-mini-language)
8. [Hot-reload flow](#hot-reload-flow)
9. [Preset state machine & auto-advance](#preset-state-machine--auto-advance)
10. [Lifetime / ownership notes](#lifetime--ownership-notes)

---

## Thread topology

Three long-lived execution contexts exchange data through lock-free, single-producer-single-consumer structures. No mutexes on the hot path.

```
 ┌─────────────────┐   ringbuf::HeapRb<f32>   ┌────────────────────┐   triple_buffer   ┌──────────────────┐
 │ cpal audio cb   │ ───────────────────────▶ │ analysis thread    │ ────────────────▶ │ winit event loop │
 │ (WASAPI driver) │    mono f32 samples      │ (seoul-analysis)   │  AudioFeatures    │ (render thread)  │
 └─────────────────┘    RING_CAPACITY=16_384  └────────────────────┘                   └──────────────────┘
```

- **`ringbuf` (SPSC, lock-free)** — chosen because the audio callback is real-time and must never block. `producer.try_push` drops samples on overflow rather than stalling the driver thread.
- **`triple_buffer` (SPSC, wait-free)** — the analysis thread writes a full `AudioFeatures` snapshot; the render thread reads whatever is most recent. No torn reads, no blocking, at most one frame of staleness.

---

## Audio capture (WASAPI loopback)

File: `src/audio/capture.rs`

Loopback on WASAPI has a specific quirk: cpal does **not** expose a dedicated loopback API. Instead, the backend detects the pattern "call `build_input_stream` on the *default output device* with the device's `default_output_config()`" and internally sets `AUDCLNT_STREAMFLAGS_LOOPBACK`. If you use `default_input_config()` or the default input device, you get microphone input instead of desktop audio. This is the single most brittle thing in the codebase.

Three sample-format code paths (`F32`, `I16`, `U16`) each push downmixed mono into the ring. `push_mono_*` averages channels into a single sample per frame. Overflow is silently dropped — that's preferable to stalling the audio callback.

---

## Feature extraction

File: `src/audio/analysis.rs`

Runs on its own thread (`seoul-analysis`), sleeping 4 ms between batches. The rolling buffer holds the last `FFT_SIZE = 2048` samples; only when `MIN_NEW_SAMPLES = 256` fresh samples have accumulated does an analysis pass run — that caps work to roughly one pass per ~5 ms at 48 kHz.

Each pass:

1. **Unroll** the ring into a linear scratch buffer (oldest → newest).
2. **Waveform** snapshot — last 512 samples copied unwindowed, for oscilloscope-style shaders.
3. **RMS volume** over the 2048 samples.
4. **Window + FFT** — Hann window applied sample-wise, then `realfft` forward R2C transform into `mags` (the norms of the complex output).
5. **Running peak normalization** — `running_peak *= PEAK_DECAY (0.9995)`, then lifted by the current frame's max. All spectrum/band values are divided by this peak so loud and quiet music share the same visual dynamic range without instantly crushing transients. The peak takes a few seconds to re-settle when content changes — that's intentional.
6. **Log-spaced spectrum** — `build_log_bin_map` precomputes a table mapping each of 64 output bins to a contiguous range of FFT bins between 50 Hz and `min(Nyquist, 20 kHz)`. Each output bin averages (not sums) the underlying magnitudes, so bin width doesn't affect amplitude.
7. **Band energies** — `bass` (60–250 Hz), `mid` (250–4000), `treble` (4k–Nyquist), computed from `mags` and peak-normalized. LERP-smoothed (`SMOOTHING = 0.35`) into the persistent `features` so visuals don't jitter.
8. **Beat detection** — 64-slot circular history of smoothed bass. A beat fires when `bass > history_avg * BEAT_THRESHOLD_RATIO (1.4)` AND `bass > BEAT_MIN_ENERGY (0.12)`. `features.beat` snaps to 1.0 on trigger and decays by `BEAT_DECAY = 0.85` per pass — giving shader authors a short exponential tail to drive strobe effects.
9. **Publish** — `out.write(features)` flips the triple buffer. The renderer sees this frame on its next read.

---

## The `AudioFeatures` shared GPU type

File: `src/audio/features.rs`

```rust
#[repr(C)]
#[derive(Pod, Zeroable)]
pub struct AudioFeatures {
    pub bass, mid, treble, volume, beat, time: f32,
    pub _pad: [f32; 2],                    // std140-style padding before arrays
    pub spectrum: [f32; SPECTRUM_BINS],    // 64
    pub waveform: [f32; WAVEFORM_SAMPLES], // 512
}
```

This struct is **simultaneously**:
- A value read out of `triple_buffer` on the CPU.
- The exact byte layout of the storage buffer at `@group(0) @binding(0)` in every composite shader (declared in `shaders/composite_prelude.wgsl`).

Any change to field order, type, padding, or array size **must** be mirrored in the prelude. `_pad: [f32; 2]` exists because WGSL aligns arrays to 16 bytes; without it, `spectrum` would start at offset 24 on the CPU but offset 32 on the GPU.

---

## Render pipeline

File: `src/render/renderer.rs::render`

Each frame runs three passes, all writing to two ping-pong feedback textures held by `render/feedback.rs`:

- `FEEDBACK_WIDTH × FEEDBACK_HEIGHT = 1280 × 720`, `Rgba16Float`. The 16-bit float format lets additive composites go well above 1.0 before clamping — essential for HDR-style bloom.
- `FeedbackTextures::new` explicitly clears both textures via a one-off render pass; wgpu does not guarantee zero-init for render targets and first-frame sampling would otherwise read garbage.
- `read_index()` / `write_index()` are XOR pairs. Every frame writes to whichever isn't being read. After submission, `swap()` XORs the index so next frame's warp pass samples what this frame just wrote.

### Pass 1 — Warp

Files: `src/render/warp.rs` + `shaders/warp.wgsl` + `src/render/mesh.rs`

The feedback-trail effect. A 48×36 grid mesh has fixed clip-space positions spanning `[-1,1]` but **per-frame dynamic UVs**. Each frame, CPU code in `WarpMesh::update` walks every vertex and computes where to sample the previous feedback texture:

1. Start from identity grid UV `(u0, v0)`. Flip V to match top-left-origin texture space vs. bottom-left-origin clip space.
2. Center-relative: `(cx, cy) = uv - 0.5`.
3. **Zoom** — divide by `zoom`. Values >1 zoom in: each output pixel reads from closer to center in the source, so the image appears to grow outward on the trail.
4. **Rotation** — multiply by the 2D rotation matrix built from `sin_cos(rotation)`.
5. **Warp** — add a low-frequency sinusoidal offset `warp_amount * sin/cos(pos * 3 + time * 0.5/0.7)`. The offset is derived from the vertex's *position*, so it's a fixed spatial pattern that slowly phase-shifts in time — swirling ripples, not uniform translation.
6. Re-center: `uv = rotated + 0.5 + warp_offset`.

The whole vertex array is then uploaded via `queue.write_buffer`. The fragment shader is trivial: `textureSample(prev, samp, uv) * decay`. Linear sampling on the 16-bit float texture handles inter-pixel blending; clamp-to-edge addressing prevents wrap-around smearing at screen borders. The `decay` uniform (clamped `[0.5, 0.9999]` by the library) determines how fast trails fade.

### Pass 2 — Composite

Files: `src/preset/shader.rs` + every preset's `.wgsl`

The **preset's own fragment shader**, drawn as a fullscreen triangle additively on top of the warped feedback:

```
blend = BlendState {
    color: { src_factor: Constant, dst_factor: One, op: Add },
    alpha: OVER,
}
```

With `src_factor = Constant, dst_factor = One`, the per-draw **blend constant** acts as a scalar multiplier on the preset's output before it's added to what's already in the target. The renderer sets it via `rpass.set_blend_constant` per draw:

- **Stable state** — one draw at intensity 1.0.
- **Transitioning state** — two draws: old preset at `1.0 - progress`, new preset at `progress`. True additive crossfade with no intermediate render target needed.

The composite target is the *same* feedback texture the warp pass just wrote, with `LoadOp::Load` — the preset draws directly on top of the warped trail. That's why the visual has layered depth: each frame's new content is permanently recorded into the trail history.

### Pass 3 — Blit

Files: `src/render/blit.rs` + `shaders/blit.wgsl`

Copies the feedback texture to the swapchain with aspect-correct letterboxing. Math in UV space:

```
if (win_aspect > SOURCE_ASPECT) { p.x *= win_aspect / SOURCE_ASPECT; } // pillarbox
else                            { p.y *= SOURCE_ASPECT / win_aspect; } // letterbox
```

`SOURCE_ASPECT` is hardcoded to `16/9` matching the feedback texture. Out-of-range UVs return black bars. The blit pass clears to black so the bars are clean even on resize.

---

## Preset system

Files: `src/preset/`

### What a preset is

A preset is a pair of files in `presets/`:

- `name.toml` — metadata, four expressions for the warp/decay parameters, an optional 4-color palette.
- `name.wgsl` — **only** the fragment function `fs_composite(in: Varying) -> @location(0) vec4<f32>`.

The WGSL file never declares bindings or helpers. `shaders/composite_prelude.wgsl` is concatenated in front of every preset's source before compilation and provides:

- `AudioFeatures` struct + `@group(0) @binding(0) var<storage,read> u: AudioFeatures`
- `Palette` struct + `@group(1) @binding(0) var<uniform> palette: Palette`
- `Varying { pos, uv }` and `@vertex fn vs_fullscreen` that generates a fullscreen triangle
- `TAU`, `PI` constants
- Helpers `waveform_at(theta)` and `spectrum_at(t)` that index the audio arrays with proper clamping

### Load pipeline

`PresetLibrary::load(device, dir, layouts)`:

1. `scan_directory` reads every `*.toml` in `dir`. `PresetSpec::load` deserializes the TOML, then parses each of the four mapping strings with `preset/expr.rs::parse` into an `Expr` AST. Bad expressions fail loading *that* preset but don't abort the library — other presets still load and a warning logs.
2. For each `PresetSpec`, `Preset::build` reads the composite WGSL, calls `compile_composite_shader` (which wraps shader module creation in `device.push_error_scope(Validation)` + `pop_error_scope` so a syntax error returns `Err` instead of panicking the device), then builds a `RenderPipeline` and a palette uniform buffer + bind group.
3. If at least one preset survives, the library is constructed. Starting preset is the one named `default` if present, otherwise first alphabetically.
4. `watcher::spawn(dir)` starts a `notify::RecommendedWatcher` whose callback filters to `.toml`/`.wgsl` modify/create events and forwards paths via `mpsc::channel`. The watcher is kept alive in a field; dropping it would stop file events.

---

## Mapping mini-language

File: `src/preset/expr.rs`

Four fields under `[mapping]` — `zoom`, `rotation`, `warp_amount`, `decay` — are strings in a tiny expression language parsed via recursive descent:

```
expr    = term (('+' | '-') term)*
term    = factor (('*' | '/') factor)*
factor  = '-'? primary
primary = number | ident | ident '(' args ')' | '(' expr ')'
```

**Variables**: `bass mid treble bass_att mid_att treble_att volume beat time dt frame bpm beat_phase beat_count aspect`. The `*_att` variants are ~1 s attenuated averages of their bands (MilkDrop semantics).

**Functions**: `sin cos abs sqrt` (arity 1), `pow min max` (arity 2), `clamp mix` (arity 3).

Parsed once at load time into an `Expr` tree. Evaluated every frame via `Expr::eval(&EvalContext { features })` — a plain AST walk, no codegen, but trivially cheap at 4 expressions × 60 Hz.

Example from `presets/default.toml`:
```
zoom        = "1.0 + bass * 0.05"
warp_amount = "0.010 + beat * 0.030"
decay       = "0.960 + treble * 0.015"
```

---

## Hot-reload flow

Files: `src/preset/watcher.rs` + `library.rs::poll_reloads`

Called from the renderer at the top of every frame, so reloads apply before that frame's evaluation:

1. Drain all pending `ReloadEvent`s from the channel into a `HashSet<PathBuf>` — editors often fire 2–3 events per save (write + modify + metadata), the set dedupes.
2. For each unique changed path:
   - **`.toml`** — find the preset whose `source_path` has the same filename. `PresetSpec::load` it, build a new `Preset` via the existing layouts, and replace the slot. This rebuilds shader, pipeline, palette buffer, and re-parses all expressions.
   - **`.wgsl`** — find *every* preset whose `composite_path` points to that filename (multiple presets can share a shader), recompile the shader module, rebuild just the pipeline. Existing `PresetSpec` and palette are preserved.
3. If compilation fails, the old preset is kept and a warning logs. Visuals never break from bad edits — you just keep the last good version until you fix it.

---

## Preset state machine & auto-advance

File: `src/preset/transition.rs`

```rust
enum PresetState {
    Stable { current: usize },
    Transitioning { from: usize, to: usize, progress: f32 },
}
```

- `begin_transition(target)` — if already transitioning, the current `to` becomes the new `from` and `progress` resets to 0. Rapid key mashing chains cleanly rather than getting stuck mid-interpolation.
- `tick(dt)` — advances `progress` by `dt / TRANSITION_DURATION (2.0)`. When `progress ≥ 1.0`, state collapses to `Stable { current: to }`.
- `frame_plan(features)` consumes this state to produce the 1- or 2-draw plan for pass 2, and LERPs all four warp/decay expression values when transitioning.

**Auto-advance** — togglable via `A`. While on and in `Stable`, every frame checks:

- `features.time - last_advance_time > AUTO_ADVANCE_INTERVAL (25 s)` **and**
- `features.beat > AUTO_ADVANCE_BEAT_THRESHOLD (0.7)`.

The beat gate makes transitions land on music rather than silence between tracks.

---

## Lifetime / ownership notes

- `RenderContext::window` is kept as `Arc<Window>` solely to anchor the `'static` lifetime of `wgpu::Surface`. Surfaces hold a raw window handle; dropping the `Arc` too early would invalidate the surface.
- Several fields carry `#[allow(dead_code)]` because they're load-bearing for lifetimes: GPU textures backing views, the `notify::RecommendedWatcher`, palette buffers backing bind groups. Don't remove them.
- `WarpUniforms` in `warp.rs` uses `[f32; 3]` padding (not `vec3`) to match the 16-byte Rust struct — see the comment in `shaders/warp.wgsl`. WGSL would otherwise align `vec3<f32>` to 16 bytes and balloon the struct to 32 bytes.
