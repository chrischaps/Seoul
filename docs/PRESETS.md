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

[params]                           # optional; named values for your shaders
spin  = "time * 0.1"                              # plain expression
lean  = { follow = "bass * 0.3", tau = 0.4 }      # eases toward it (τ in s)
depth = { rate = "0.05 + bass_att", wrap = 1.0 }  # integrates per second

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
- **Drums:** `kick snare hat` jump to 1 on that instrument's onset and
  decay in 0.05–0.12 s; `kick_age snare_age hat_age` are seconds since its
  last hit (good for things that spread or fade, like ripples);
  `kick_count snare_count hat_count` number the hits, so
  `rand2(vec2<f32>(u.snare_count, 1.0))` gives each hit its own variation.
- **Soba link:** `cv1`…`cv8` (control lanes, 0..1 or −1..1),
  `gate1`…`gate4` (0/1) with `gate1_age`… and `gate1_count`… like the
  drums, `link_live`, `playing` and `bar`. A patch plays these over OSC
  (see [LINK.md](LINK.md)). With no link they read 0 (ages 1000), so blend
  toward an audio feature: `mix(bass_att, cv1, link_live)`. Gayageum and
  Obangsaek are built this way.
- **Functions** (WGSL semantics): `sin cos tan abs sqrt exp log floor fract
  sign pow min max atan2 step clamp mix smoothstep`.

### `[params]`

Up to 16 named values, evaluated every frame in file order, for anything
your shaders need that `u` doesn't give you: a fractal constant, a zoom
depth, a smoothed copy of a band. Three kinds:

- **`"expr"`** (or a number): the expression's value.
- **`{ follow = "expr", tau = 0.25 }`**: eases toward the expression with
  time constant `tau` seconds. Starts at its target, so nothing fades in
  from zero. Use it to make audio-driven shapes glide instead of twitch.
- **`{ rate = "expr", wrap = 1.0 }`**: adds up the expression over time
  (units per second), so speed can follow the music without the position
  jumping. `wrap` keeps it in `0..wrap` for seamless loops (`6.2831853` for
  an angle).

A param can use any param declared above it, and `[mapping]` and `[post]`
can use all of them. Names are lowercase identifiers that aren't a built-in
variable, function or WGSL keyword. `follow` and `rate` are per second, so
they behave the same at any refresh rate. Values reset when the preset
reloads.

In WGSL, `params()` returns a struct with one `f32` field per param:

```wgsl
let q = params();
let c = vec2<f32>(q.cx, q.cy);
```

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
`u.waveform[512]`, `u.resolution`, and the link's `u.cv[8]`, `u.gate[4]`,
`u.gate_age[4]`, `u.gate_count[4]`, 0-based), `palette.colors[0..3]`, `TAU`, `PI`,
and helpers:

| Helper | |
|---|---|
| `waveform_at(theta)` | waveform around a circle (seamless) |
| `waveform_lin(t)` | waveform left→right, `t` in 0..1 |
| `spectrum_at(t)` | spectrum low→high, interpolated |
| `spectrum_ago(t, seconds)` | the spectrum as it was up to 8.5 s ago — draw the recent past (see Bukhansan Ridges, Jeongdongjin Sunrise) |
| `palette_ramp(t)` | smooth ramp through the 4 palette colors |
| `hue_rotate(c, a)`, `luma(c)` | color utilities |
| `rand2(p)`, `vnoise(p)`, `fbm(p)` | hash, value noise, 4-octave fractal noise |
| `curl_noise(p)` | divergence-free 2D flow |
| `params()` | this preset's `[params]` (if it declares any) |

### Fractal helpers

Complex numbers are `vec2<f32>` (x real, y imaginary).

| Helper | |
|---|---|
| `cmul cdiv csq csqrt conj cabs2 cexp clog cpow(z, n)` | complex arithmetic |
| `julia_beta(c)` | the repelling fixed point of z² + c (on the Julia set; multiplier 2z) |
| `disk_move(z, a)` | Möbius move of the unit disk taking 0 to `a` (hyperbolic glide) |
| `rot2(a)` | 2×2 rotation matrix |
| `fold_plane(p, n)`, `circle_invert(p, c, r)` | folds and inversions |
| `segment_dist(p, a, b)` | distance to a segment (for fold-built trees) |
| `julia(z0, c, max_iter, trap_r)` | escape time → `(smooth count, ring trap, escaped, min \|z\|)` |
| `kali(p, c, iters)` | Kali set → `(accumulated \|p\| change, min \|p\|)` |
| `cosine_palette(t, phase)` | seamless cycling through the palette |

`presets/julia_bloom.wgsl` (escape time), `presets/indras_pearls.wgsl`
(circle inversions), `presets/kali_nebula.wgsl` and `presets/sonamu.wgsl`
(a fold-built tree) are worked examples.

### Endless dives

f32 runs out of precision after a few thousand × of zoom, but a
self-similar fractal never needs more than one level of it. Zoom by exactly
the fractal's own scale factor while a `rate` param with `wrap = 1.0` runs
from 0 to 1, and the frame at 1 matches the frame at 0:

- **IFS:** view the system through `f_k^(−dive)`, a fractional power of one
  map (`presets/hutchinson_warp.wgsl`).
- **Julia sets:** zoom on the repelling fixed point by its multiplier λ.
  Starting a few iterations early (`z* + u·λ^−N`, then N steps of z² + c)
  makes the self-similarity exact, and `count − dive` keeps the colors
  continuous across the wrap (`presets/julia_bloom.wgsl`).
- **The Mandelbrot set:** dive toward a Misiurewicz point c₀ (its orbit
  lands on a repelling cycle), zooming by that cycle's multiplier. Iterate
  as a perturbation δ around c₀'s own orbit, which is known exactly, and
  rebase onto the start of the orbit when |z| < |δ|; then even a deep zoom
  stays sharp in f32 (`presets/mandelbrot_dive.wgsl`).
- **Pictures that contain themselves (Droste):** draw one level between an
  outer edge and an opening 1/S its size, then take `clog(p)`, multiply by
  α = 1 + ln S/(2πi) (Escher's spiral), slide by `dive · ln S` and wrap
  into one level before `cexp` (`presets/droste_window.wgsl`).
- **Phyllotaxis:** seeds at radius gⁿ, n golden angles round, are carried
  seed to seed by scaling by g. Advance a Fibonacci number of seeds per
  cycle and they stream outward instead of spinning
  (`presets/sunflower.wgsl`).
- **Anything else:** crossfade a few zooming copies a fraction of a cycle
  apart with sin² weights (`presets/kali_nebula.wgsl`), or move objects
  through a perspective divide (`presets/sonamu.wgsl`,
  `presets/hongsalmun.wgsl`).

A feedback warp built from an expanding map (z² + c) can't zoom in: near
the set it samples outside the zoomed window. Use the `[post]` video echo
for depth there instead.

**Scene kit**, for drawing places and things (distances in `centered`
units, negative inside):

| Helper | |
|---|---|
| `sd_circle(p, r)`, `sd_box(p, half)`, `sd_segment(p, a, b)` | signed distances |
| `fill(d)` | antialiased 1-inside / 0-outside of a distance |
| `glow(d, r)` | soft light falloff outside a shape |
| `ridge(x, seed, rough)` | mountain ridgeline height 0..1 |
| `hanok_roof(x)` | tiled-roof profile for `x` in −1..1 (−1 outside) |
| `stars(p, scale, density)` | static starfield |
| `rain(p, t, density, slant)` | falling streaks |

Because the composite is added on top of the warped past, a solid object
"occludes" simply by being what this pixel returns: draw back to front,
replacing `col` rather than adding to it. See `presets/jongno_rain.wgsl`.

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
frame-rate normalized), `u`, `palette`, and all the common helpers, plus
`water_reflect(uv, horizon_t, stir, phase)`: the scene above `horizon_t`
mirrored into rippling water below it. See `presets/ink_warp.wgsl` (fluid
advection), `presets/han_river_warp.wgsl` (water reflection) and
`presets/jongno_rain_warp.wgsl` (reflection plus kick ripples) for
complete examples.

A warp can also *be* the fractal. Gather last frame through a few
contracting maps and keep the brightest, and the feedback loop runs an
iterated function system, converging onto its attractor
(`presets/hutchinson_warp.wgsl`). Sample last frame at `z² + c` and it runs
the Julia iteration, one step per frame (`presets/julia_feedback_warp.wgsl`).
Both need `edge = "fade"` so light that leaves the screen is gone.

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
