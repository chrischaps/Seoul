# Seoul — Implementation Guide

How Seoul turns system audio into feedback-driven visuals: threads, the
analysis math, the per-frame GPU pass graph, the preset system, and the
invariants that keep it all consistent. For writing presets, see
[`PRESETS.md`](PRESETS.md).

## Table of contents

- [Thread topology](#thread-topology)
- [Audio capture](#audio-capture)
- [Feature extraction](#feature-extraction)
- [Room tuning](#room-tuning)
- [`AudioFeatures`: CPU snapshot and GPU layout](#audiofeatures-cpu-snapshot-and-gpu-layout)
- [Timing model](#timing-model)
- [Render pass graph](#render-pass-graph)
- [Transitions and masks](#transitions-and-masks)
- [Preset system](#preset-system)
- [Hot reload](#hot-reload)
- [Curation, auto-advance, config](#curation-auto-advance-config)
- [Frame mode](#frame-mode)
- [HUD](#hud)
- [Verification tooling](#verification-tooling)
- [Invariants checklist](#invariants-checklist)

---

## Thread topology

```
 WASAPI callback ──ring (f32 mono)──▶ analysis thread ──triple buffer──▶ render thread
 (cpal, realtime)                     (Analyzer, ~5.8 ms hop)            (winit loop, wgpu)
        ▲                                     ▲
        └─────── Capture (main thread) ───────┘  sends a new ring on (re)connect

 Soba (OSC/UDP 127.0.0.1:9137) ──▶ link thread ──snapshots (mpsc)──▶ render thread
                                   (LinkState, gate edges)         (merged after `delay_ms`)
```

- **Audio callback** (`audio/capture.rs`): downmixes any sample format
  (F32/F64/I16/I24/I32/U16) to mono f32 and pushes into a `ringbuf::HeapRb`.
  Drops samples rather than blocking if analysis falls behind.
- **Analysis thread** (`audio/analysis.rs`): drains the ring, runs the
  `Analyzer`, publishes `AudioFeatures` into a `triple_buffer`. It receives
  new `AudioSource`s (ring + sample rate) over an mpsc channel whenever
  capture reconnects, rebuilding the analyzer if the rate changes.
- **Render thread** (`main.rs`, `render/`): owns the window, polls capture,
  reads the latest features, renders. `--synth` replaces capture with a
  generator thread (`audio/synth.rs`) feeding the same channel.
- **Link thread** (`audio/link.rs`): receives the Soba link's OSC on
  `127.0.0.1`, folds messages into a `LinkState` (counting gate rising
  edges at the full 200 Hz) and sends a timestamped snapshot per packet.
  The render thread keeps ~2 s of them and merges the one from
  `[link] delay_ms` ago into its copy of the features, right after reading
  them. The merge happens there, not in analysis, because `delay_ms` must
  be measured on the presenting clock and `--record` runs analysis on the
  render thread. A live, playing link overrides `bpm`, `beat_phase`,
  `beat_count`, `bar` and `bpm_confidence`. Protocol and semantics are in
  `docs/LINK.md`.

## Audio capture

cpal implements WASAPI loopback implicitly: `build_input_stream` on a
*render* device, with the config from `default_output_config()`, makes cpal
set `AUDCLNT_STREAMFLAGS_LOOPBACK`. This is still true in cpal 0.18.
cpal's ALSA backend on Linux has no loopback, so there `Loopback` fails
with a message pointing at `--mic` and at PulseAudio/PipeWire monitor
inputs, and `Capture` keeps retrying it like any unavailable device.

`Capture` opens a `Source`: `Loopback` (the default output, as above) or
`Input` — the default input device, or the first whose name contains some
text (`--mic`, `--input <NAME>`, `[audio]` in `seoul.toml`; `--list-inputs`
prints the names). An input uses `default_input_config()` and goes through
the same downmix. Inputs are flagged for [room tuning](#room-tuning) unless
`[audio] room = false` or `--no-room`.

`Capture` lives on the main thread (cpal streams are `!Send`) and is polled
every frame:

- The stream's error callback sets an atomic flag for anything but `Xrun`.
  cpal 0.18 reports `StreamInvalidated` / `DeviceNotAvailable` when the
  default device changes; the flag triggers a reconnect after 250 ms.
- Every 2 s it also compares the default device's id with the active one
  (belt and braces). A device picked by name isn't followed; if it goes
  missing, the retry below finds it when it's plugged back in.
- With no device, the app runs on silence and retries every 2 s.
- A config reload that changes `[audio]` reopens capture (`set_source`).

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
| `kick`, `snare`, `hat` (+ `*_age`, `*_count`) | one detector per instrument, same peak picker as `beat` plus a floor of a fraction of the band's recent peak: kick = *linear*-magnitude flux 30–180 Hz (log flux would fire on bass beating against a kick's tail), snare = linear flux 1–4 kHz (hats leak ~20 dB below claps there), hat = log flux 7–16 kHz. A hat within 40 ms of a snare, in either order, is retracted as the clap's own noise. `*_age` is seconds since the last hit (1000 before any); `*_count` lets shaders hash each hit |
| `bpm`, `bpm_confidence` | inter-onset-interval histogram over the last 10 s (pairs 0.25–2 s apart, folded into 80–160 BPM, Gaussian-smeared, parabolic peak); confidence = mass near the peak |
| `beat_phase` | oscillator at `bpm`, nudged toward 0 when a detected beat lands near it |
| `waveform[512]` | trigger-aligned to the steepest rising zero crossing, auto-gained, 24-sample sin² taper at both ends (so circular plots close), light temporal smoothing |

**Silence.** WASAPI loopback delivers *no packets* while nothing plays. If
nothing arrives for 40 ms, the thread feeds real-time zeros, so every
envelope decays exactly as it would on digital silence; nothing latches.

Tests (`audio::analysis::tests`) cover silence, sub-floor noise, kick
detection at 100/120/140 BPM, envelope decay, waveform stability, taper,
and per-band balance.

## Room tuning

A microphone across the room hears the music 30 dB or more down, with
little bass, over a steady bed of noise (fans, traffic, its own hiss).
Untuned, that fails one of two ways depending on the mic's gain: a quiet
mic puts the music under the −50 dBFS floor and every band reads zero (the
picture sits dark, though tempo still locks — log flux is level-blind); a
hot mic puts the room's noise above the floor, and the AGC stretches it
into a picture that twitches in an empty room.

`Analyzer::set_room(true)` adds a `Room` stage between the FFT and
everything that reads it:

1. **Noise floor per bin.** Each bin's power is smoothed (τ 100 ms) and a
   floor is tracked beneath it: it falls toward quieter moments (τ 0.5 s)
   and rises under steady sound at a rate set by whether anything is
   happening. The room is what doesn't change: after 3 s with no detected
   onset (a fan switching on) it rises at 1 dB/s; while onsets keep coming
   it creeps at 0.05 dB/s, so a record's pad and bassline aren't taken for
   the room. The first 0.5 s primes it (the window has only just filled),
   and digital silence — a muted mic — holds it rather than teaching it
   zero. 1.5× the floor is subtracted from every bin's power before bands,
   spectrum and onsets see it. `presence` is the share of the smoothed
   power above that (0..1, square-rooted); a single hop's power swings too
   far above its floor to tell the room from music.
2. **Gain, lifted or cut.** The gain puts the room's noise floor (its
   loudest band) 3 dB *under* the analysis floor: lifting a quiet mic,
   cutting a hot one (−40…+36 dB, easing over ~3 s up and 0.5 s down).
   Every absolute threshold downstream then means "above the room",
   whatever the mic's gain. It depends on the room alone, so loud talk
   can't hold it down for the record that follows.
3. **The spectrum is measured against the room.** The direct feed's
   absolute scale (a −35 dB reference floor) doesn't carry over: a room's
   music sits 30–40 dB lower per bin than in its bands, which sum many
   bins, and its noise piles up at one end. Each of the 64 bins instead
   shows its smoothed power over its learned floor, mapped from 3 dB above
   it (nothing) to 21 dB (full), so an empty room reads zero everywhere.
4. **Presence gates the rest.** `volume` comes from raw samples, so it's
   scaled by `presence`. The waveform (whose own AGC would draw hiss full
   height) and onset strengths are multiplied by `smoothstep(0.3, 0.7,
   presence)`, since what's left of the noise after subtraction still
   flickers and log flux turns flicker into beats.

The trade-off: a drone with no onsets at all is taken for the room within
about 20 s. A test plays three minutes of record through the room and
checks the third minute reads like the first (an earlier version, with a
single faster creep rate and frame-level presence, lost most of the
record's volume and its onset gate within a minute).

`RoomSim` (`audio/synth.rs`, `--room-sim`) is the test bench: the synth
track 30 dB down, two 150 Hz high-passes, four early reflections, rumble
and hiss at about 20 dB SNR, all scaled by a mic gain. The tests run it at
−20, 0 and +20 dB and check that the untuned failure is real, that tuned
it locks 124 BPM with moving bands, that an empty room stays dark with no
beats at every gain, that quiet and hot mics hear the same record, and
that a long record doesn't fade into the room.
`room_report` (ignored; `cargo test --release room_report -- --ignored
--nocapture`) prints the numbers behind them. The HUD's stats panel shows
the gain and floor, "learning the room…", or "no signal — is the mic
muted?" after 1 s of digital zeros.

## `AudioFeatures`: CPU snapshot and GPU layout

`AudioFeatures` (`audio/features.rs`) is `#[repr(C)] Pod`: a 56-scalar header
(including `resolution: vec2` on a 16-byte boundary, then rows of drum
envelopes, ages and counts, each padded to a vec4, then the link's
`cv[8]`, `gate[4]`, `gate_age[4]`, `gate_count[4]` and a
`link_live, playing, bar` row), then
`spectrum[64]` and `waveform[512]`. It is uploaded verbatim to a storage
buffer bound by every composite, warp and particle pipeline. The WGSL
mirror lives in **`shaders/common.wgsl`** and must match field for field; a
`const` size assertion in Rust guards drift.

## Spectrum history

`render/history.rs` keeps the last 8.5 s of spectrum as a 64×256 `R32Float`
ring texture, one row per 1/30 s of *render* time (frame-rate independent
and deterministic under `--record`). It is bound beside `u` in the shared
audio bind group (binding 1; each prelude declares `hist`). The render
thread stamps `u.hist_pos` = newest row + progress toward the next, so
`spectrum_ago(t, seconds)` (in `common.wgsl`) scrolls smoothly; it reads
with `textureLoad` and interpolates by hand, since 32-bit float textures
aren't filterable without an optional feature.

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
- Scene time runs at `speed` × real time (`Renderer::set_look`; 1 outside
  frame mode). Because every motion, decay, particle step and transition
  goes through `dt`, a slower `speed` is true slow motion with the same
  equilibrium brightness. The HUD keeps real time.

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

Preludes are `common.wgsl` + `fractal.wgsl` (complex math, escape time,
folds, inversions) + the stage prelude, plus a per-preset generated snippet.

**`[params]`** (`preset/params.rs`): up to 16 named per-frame values,
parsed from a raw `toml::Table` (`toml`'s `preserve_order` keeps file
order). Each is a plain expression, a `follow` (exponential approach, τ in
seconds) or a `rate` (integrated per second, optional `wrap`); per-second
units make them frame-rate independent, and steps are capped at 0.1 s so a
stall can't fling an integrator. Expressions see earlier params as
`Expr::Param(i)` (`expr::parse_with`). Live values (`ParamState`) sit on
the `Preset` and reset on rebuild. `PresetLibrary::tick_params` advances
the on-screen presets once per frame (before `frame_plan`, which evaluates
`[mapping]` and `[post]` with the values in scope) and writes them into the
preset's uniform buffer, which is the palette buffer extended to
`PresetUniforms { colors, params }` (128 B). Shaders read them through a
generated `struct Params` + `fn params()` appended to the prelude, before
the author's body, so error line remapping is unaffected.

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
  (global look defaults), `[hud]`, `[audio]`, `[frame]`, `[link]`, plus
  start preset, fullscreen/monitor, render scale. CLI flags override.
- `LibrarySettings::favorites_only` (frame mode's default playlist) makes
  `visible()` admit only favorites, once any exist, so Space, R and
  auto-advance all keep to them.

## Frame mode

`--frame` or `[frame] enabled` turns Seoul into a picture on the wall:
borderless fullscreen with no cursor, the 서울 wordmark at startup and
then no HUD (`Hud::wordmark_only`; F1 still works), auto-advance forced
on, favorites as the playlist. `frame.rs` holds the logic, pure and
unit-tested:

- **`MoodTracker`** reads the analysed features each frame. *Resting*:
  nothing above `volume` 0.15 for 4 s. *Playing*: music for 2 s, where
  music is any of: 4 s of unbroken sound (no gap of 0.15 s; talk breaks
  between phrases every 1–3 s, a record doesn't), sound ~90% of the last
  ~8 s, a confident tempo with recent beats, or a live, playing Soba link.
  It stops 5 s after the evidence does. *Listening*: anything else.
- **`FrameMode`** eases a `Look` toward each mood — resting: speed 0.25,
  exposure 0.4, presets hear nothing; listening: 0.5, 0.65, they hear the
  music at 35% and no beats (drum ages read 1000, so hit-triggered effects
  stay still); playing: everything at 1 — waking over ~1.5 s and settling
  over ~6 s. From `night` it adds a warm grade (`PostUniforms::warmth`,
  over ~60 s) and ×0.75 exposure; from `sleep` to `wake` it fades to black
  (τ 10 s) unless music is playing, and the render loop drops to a few
  frames a second once dark. The first frame snaps to the hour, so a frame
  switched on at 2 am starts dark.
- The time of day is local (`chrono`), or `--clock HH:MM` from there on.
  Recordings default to noon, so they repeat.

`--evening` plays a scripted room for testing it (`TestTrack::evening`):
empty, then 20 s of `Talk` (formant-filtered glottal pulses in phrases of
4–11 syllables with pauses between, so it has speech's rhythm and gaps),
then the record from its first bar, then empty. A test runs it through
the room-tuned analyzer and asserts each stretch's mood; `mood_report`
(ignored) prints the timeline and the longest unbroken run during talk
(2.2 s against the 4 s threshold).

## HUD

`render/hud.rs` draws after tonemapping, so it never blooms. Text is
glyphon/cosmic-text over fonts loaded straight from the OS's font folder
(`render/hud.rs::os`): Bahnschrift, Malgun Gothic for Hangul and Consolas
on Windows; Avenir Next, Apple SD Gothic Neo and Menlo on macOS; Noto Sans,
Noto Sans CJK and DejaVu Sans Mono on Linux. A full system scan is the
fallback. Text buffers re-shape only when their content changes;
panels and meters are instanced SDF rounded rects (`shaders/hud.wgsl`)
with premultiplied blending. Elements: startup 서울 wordmark, preset toasts
(palette-colored accent, drop shadow), key notices, help (H), stats (F1:
FPS, tempo, beat phase, band meters with ~1 s average ticks, kick/snare/hat
lamps, spectrum strip, audio device), and the preset error panel. `P` screenshots include
the HUD (the post and HUD passes are re-run offscreen for the last frame).

## Verification tooling

- `--synth`: deterministic 124 BPM test track (kick, clap, hats, ducked
  bass, pad, arpeggio, kick-less breakdown every eighth bar), plus a
  stand-in Soba link playing along over real UDP (`audio/link_fake.rs`;
  `--no-link` skips it). `--record` folds the same lanes in directly.
- `--tour SECS [--tour-shots N] [--preset filter]`: hard-cut through
  presets, screenshot each into `screenshots/tour/`, exit. The HUD is
  disabled for clean frames.
- `--bench SECS [--preset filter]`: the tour's route, timing frames
  instead (`bench.rs`). Vsync is off (`PresentMode::AutoNoVsync`) so the
  numbers show headroom; the first second on each preset isn't counted.
  It logs each preset's fps and its slowest one-second window, and saves a
  CSV, slowest first, to `screenshots/bench/`. A preset "holds 60" when its
  slowest window does. Single frame times mislead here: with vsync off the
  GPU queue fills and drains, so frames arrive in fast/slow pairs.
- `--screenshot-at SECS`, `P`: PNG of the current frame.
- `--room-sim`, `--evening`: the synth through a simulated room mic, or a
  scripted evening of empty room, talk and a record; `--frame --clock
  HH:MM` previews frame mode at any hour.

## Invariants checklist

- `AudioFeatures` (Rust) ⇔ `shaders/common.wgsl` field for field.
- `WarpUniforms` (80 B), `PostUniforms` (80 B), `ParticleUniforms` (96 B),
  mask uniforms (32 B), `PresetUniforms` (128 B, WGSL `Palette`): Rust
  structs ⇔ WGSL structs, guarded by size asserts.
- Everything drawn into the feedback that should respect transitions
  (warp, composite) uses `MASKED_BLEND` + `ColorWrites::COLOR`; nothing
  else may write feedback alpha except the mask pass.
- Per-frame quantities authored at 60 Hz must go through `k = dt·60`.
- Anything new compiled from user files goes inside an error scope.
- Link values join `AudioFeatures` on the render thread only, and every
  link field has a defined "no link" value (lanes 0, gate ages 1000).
