# Writing Seoul Presets

A preset is a small TOML file plus one or two WGSL fragment functions in
`presets/`. Save either file while Seoul is running and it reloads live; if
something's wrong, the error appears on screen with the file and line.

```
presets/
  my_preset.toml          # name, motion, palette, look
  my_preset.wgsl          # fs_composite: what gets drawn each frame
  my_preset_warp.wgsl     # optional fs_warp: how last frame moves
```

## The mental model

Every frame:

1. **Warp** — last frame's image is resampled (zoomed, rotated, shifted,
   rippled…) and multiplied by `decay`. This is where trails come from.
2. **Particles** — optional, added on top.
3. **Composite** — your `fs_composite` output is *added* on top.
4. **Post** — bloom, tonemapping, grading; display only, never fed back.

Because the composite is additive and the warp keeps a `decay` fraction of
the past, anything drawn steadily settles at roughly

```
brightness ≈ per-frame output / (1 − decay)
```

At `decay = 0.96` that's 25×. Keep per-frame contributions small (around
0.005–0.1) and let the feedback build the image. The tonemapper rolls off
highlights gracefully, but a preset that sits at near-white is a washed-out
preset. Check with `seoul --synth --tour 10 --tour-shots 3 --preset <name>`.

## The TOML

```toml
name = "Han River"                 # required, shown in the HUD
author = "seoul"
description = "One sentence for the toast."
edge = "mirror"                    # mirror (default) | fade | clamp

[mapping]                          # expressions, evaluated every frame
zoom        = "1.0 + bass * 0.02"  # required; >1 drifts outward
rotation    = "0.003"              # required; radians per frame
warp_amount = "0.002 + beat*0.01"  # required; sine wobble
decay       = "0.95"               # required; 0.5–0.9999
cx          = "0.5"                # zoom/rotation center (texture space)
cy          = "0.5"
dx          = "0"                  # translate per frame
dy          = "0"
sx          = "1"                  # stretch per frame
sy          = "1"
warp_scale  = "1"                  # wobble spatial frequency
warp_speed  = "1"                  # wobble speed
hue_shift   = "0"                  # radians per SECOND of hue drift
blur        = "0"                  # 0..1 feedback softening
sharpen     = "0"                  # feedback sharpening

[shader]
composite = "han_river.wgsl"       # required
warp      = "han_river_warp.wgsl"  # optional custom warp

[palette]                          # optional; four RGBA colors
color0 = [0.04, 0.07, 0.18, 1.0]
color1 = [1.00, 0.62, 0.25, 1.0]
color2 = [0.95, 0.35, 0.60, 1.0]
color3 = [0.92, 0.94, 1.00, 1.0]

[post]                             # optional; numbers OR expressions
bloom           = 0.75             # strength
bloom_threshold = 0.35
exposure        = 1.0
saturation      = 1.05
contrast        = 1.0
vignette        = 0.3
grain           = 0.15
chroma          = "beat * 0.5"     # chromatic aberration
led_mask        = false            # LED dot-matrix display
led_pitch       = 9.0              # LED spacing in px at 1080p
echo_alpha      = 0.4              # MilkDrop video echo
echo_zoom       = "1.3 + bass_att * 0.1"
echo_orient     = "xy"             # none | x | y | xy (flips)
mirror          = "kaleido:8"      # none | x | y | quad | kaleido | kaleido:N

[particles]                        # optional GPU particle layer
count      = 20000                 # up to 131072
spawn      = "ring"                # center | ring | edges | waveform | random
speed      = 0.15                  # initial, screen heights per second
flow       = 0.5                   # curl-noise strength
flow_scale = 2.0
drag       = 0.8                   # per second
size       = 3.0                   # px at 1080p
life       = 3.0                   # seconds (randomized ±50%)
color      = 3                     # palette index 0–3 | "ramp" | "spectrum"
burst      = 0.0                   # outward kick per detected beat
bass_push  = 0.0                   # outward push × bass
gravity    = 0.0
intensity  = 0.05
```

Motion values are written **per frame at 60 Hz**; Seoul rescales them for
the actual refresh rate, so a preset looks the same at 60 and 144 Hz.
Unknown keys are errors, so a typo never silently does nothing.

### Expression language

Arithmetic `+ - * /`, parentheses, unary minus, and:

- **Variables:** `bass mid treble volume` (0..1, auto-gained per band),
  `bass_att mid_att treble_att` (~1 s averages), `beat` (jumps to 1 on a
  beat, decays in ~0.1 s), `beat_phase` (0→1 between beats at the detected
  tempo), `beat_count`, `bpm`, `time` (seconds), `dt`, `frame`, `aspect`.
- **Functions** (WGSL semantics): `sin cos tan abs sqrt exp log floor fract
  sign pow min max atan2 step clamp mix smoothstep`.

## The composite shader

Write only `fs_composite`. Everything below is already declared; do not
redeclare it.

```wgsl
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);          // aspect-correct, origin at center
    let ring = smoothstep(0.01, 0.0, abs(length(p) - 0.2 - u.bass * 0.1));
    return vec4<f32>(palette.colors[1].rgb * ring * (0.1 + u.beat * 0.2), 1.0);
}
```

`in.uv` runs 0..1 with **y up**. Use `centered(uv)` (y in ±0.5, x in
±0.5·aspect) or `polar(uv)` → `(radius, angle)` for anything round.

**Available:** `u` (all audio/timing fields above, plus `u.spectrum[64]`,
`u.waveform[512]`, `u.resolution`), `palette.colors[0..3]`, `TAU`, `PI`,
and helpers:

| Helper | |
|---|---|
| `waveform_at(theta)` | waveform around a circle (seamless) |
| `waveform_lin(t)` | waveform left→right, `t` in 0..1 |
| `spectrum_at(t)` | spectrum low→high, interpolated |
| `palette_ramp(t)` | smooth ramp through the 4 palette colors |
| `hue_rotate(c, a)`, `luma(c)` | color utilities |
| `rand2(p)`, `vnoise(p)`, `fbm(p)` | hash, value noise, 4-octave fractal noise |
| `curl_noise(p)` | divergence-free 2D flow |

## A custom warp

Add `warp = "x_warp.wgsl"` and write `fs_warp`. It must return last frame's
image, transformed however you like, times `w.decay`.

```wgsl
@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let c = centered(in.uv);
    let flow = curl_noise(c * 2.0 + vec2<f32>(w.time * 0.03, 0.0));
    let d = flow * 0.001 * frame_k();             // per-frame motion × frame_k()
    let src = warp_uv(in.uv) - vec2<f32>(d.x / w.aspect, -d.y);
    return finish(src, sample_prev(src));         // blur/sharpen/hue/decay
}
```

Here `in.uv` is **texture space (y down)**. The warp prelude provides:
`sample_prev(uv)` (honors `edge`), `blur4(uv, r)`, `warp_uv(uv)` (the TOML
`[mapping]` transform), `finish(src, color)`, `centered(uv)` /
`uncentered(c)` (y up), `frame_k()`, the warp uniforms `w.*` (already
frame-rate normalized), `u`, `palette`, and all the common helpers. See
`presets/ink_warp.wgsl` (fluid advection) and `presets/han_river_warp.wgsl`
(water reflection) for complete examples.

## Transitions come for free

Never handle transitions in a preset. Seoul weights each preset's warp and
composite per pixel during dissolves and wipes on its own.

## Workflow

```bash
cargo run --release -- --synth --preset "My Preset"   # live-edit with a steady beat
cargo run --release -- --synth --tour 10 --tour-shots 3 --preset "My Preset"
```

Press **F1** for live meters (bands, tempo, spectrum) while tuning, and
**P** to save a screenshot.
