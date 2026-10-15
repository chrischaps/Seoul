// Shared by every Seoul shader prelude (composite, warp, particles).
// Each prelude declares the `u: AudioFeatures` and `palette: Palette`
// bindings itself; WGSL module-scope order doesn't matter, so the helpers
// below can reference them.

// Layout must match `src/audio/features.rs::AudioFeatures` exactly.
struct AudioFeatures {
    bass: f32,
    mid: f32,
    treble: f32,
    volume: f32,

    bass_att: f32,
    mid_att: f32,
    treble_att: f32,
    beat: f32,

    time: f32,
    dt: f32,
    frame: f32,
    bpm: f32,

    beat_phase: f32,
    beat_count: f32,
    aspect: f32,
    bpm_confidence: f32,

    resolution: vec2<f32>,
    pad: vec2<f32>,

    kick: f32,
    snare: f32,
    hat: f32,
    hist_pos: f32,

    kick_age: f32,
    snare_age: f32,
    hat_age: f32,
    pad3: f32,

    kick_count: f32,
    snare_count: f32,
    hat_count: f32,
    pad4: f32,

    spectrum: array<f32, 64>,
    waveform: array<f32, 512>,
};

// A preset's uniform buffer. Layout must match
// `src/preset/preset.rs::PresetUniforms`. `params` holds the preset's
// [params] values; read them by name through the generated `params()`.
struct Palette {
    colors: array<vec4<f32>, 4>,
    params: array<vec4<f32>, 4>,
};

const TAU: f32 = 6.2831853;
const PI:  f32 = 3.1415927;

// ---- Audio ----------------------------------------------------------------

// Sample the analyzed waveform at an angle in radians (typically `atan2(p.y, p.x)`).
// Linearly interpolated; the analysis tapers both ends to zero so a full
// circle closes without a seam at theta = ±PI.
fn waveform_at(theta: f32) -> f32 {
    let x = clamp((theta / TAU + 0.5) * 511.0, 0.0, 511.0);
    let i = u32(x);
    let j = min(i + 1u, 511u);
    return mix(u.waveform[i], u.waveform[j], fract(x));
}

// Sample the waveform linearly at a normalized position (0 = oldest, 1 = newest).
fn waveform_lin(t: f32) -> f32 {
    return waveform_at((clamp(t, 0.0, 1.0) - 0.5) * TAU);
}

// Sample the log-spaced spectrum at a normalized position (0 = low, 1 = high),
// linearly interpolated between bins.
fn spectrum_at(t: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0) * 63.0;
    let i = u32(x);
    let j = min(i + 1u, 63u);
    return mix(u.spectrum[i], u.spectrum[j], fract(x));
}

// The spectrum as it was `seconds` ago (0 = now, up to HIST_SECONDS), at
// `t` 0..1 low→high: a scrolling spectrogram, interpolated in both
// directions. `hist` is declared by each prelude.
const HIST_ROWS: f32 = 256.0;
const HIST_RATE: f32 = 30.0;
const HIST_SECONDS: f32 = 8.5;

fn hist_row(row: f32, x: i32) -> f32 {
    let r = i32(row - floor(row / HIST_ROWS) * HIST_ROWS);
    return textureLoad(hist, vec2<i32>(x, r), 0).r;
}

fn spectrum_ago(t: f32, seconds: f32) -> f32 {
    let s = min(u.hist_pos - clamp(seconds, 0.0, HIST_SECONDS) * HIST_RATE, floor(u.hist_pos));
    let x = clamp(t, 0.0, 1.0) * 63.0;
    let i = i32(x);
    let j = min(i + 1, 63);
    let r0 = floor(s);
    let a = mix(hist_row(r0, i), hist_row(r0, j), fract(x));
    let b = mix(hist_row(r0 + 1.0, i), hist_row(r0 + 1.0, j), fract(x));
    return mix(a, b, s - r0);
}

// ---- Color ----------------------------------------------------------------

// Smooth ramp through the four palette colors, t in 0..1.
fn palette_ramp(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 3.0;
    let i = u32(min(x, 2.999));
    return mix(palette.colors[i].rgb, palette.colors[i + 1u].rgb, smoothstep(0.0, 1.0, x - f32(i)));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Rotate hue by `a` radians (Rodrigues rotation about the grey axis).
fn hue_rotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735027);
    let cs = cos(a);
    return c * cs + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cs);
}

// ---- Noise ----------------------------------------------------------------

// Integer-hash based random in [0, 1) — stable on any GPU.
fn rand2(p: vec2<f32>) -> f32 {
    var h = vec2<u32>(vec2<i32>(floor(p))) * vec2<u32>(1597334673u, 3812015801u);
    let n = (h.x ^ h.y) * 1597334673u;
    return f32(n >> 8u) / 16777216.0;
}

// Smooth value noise in [0, 1).
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = rand2(i);
    let b = rand2(i + vec2<f32>(1.0, 0.0));
    let c = rand2(i + vec2<f32>(0.0, 1.0));
    let d = rand2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

// Four-octave fractal noise in roughly [0, 1).
fn fbm(p_in: vec2<f32>) -> f32 {
    var p = p_in;
    var sum = 0.0;
    var amp = 0.5;
    for (var o = 0; o < 4; o = o + 1) {
        sum = sum + amp * vnoise(p);
        p = p * 2.03 + vec2<f32>(17.1, 9.7);
        amp = amp * 0.5;
    }
    return sum / 0.9375;
}

// Divergence-free flow (curl of an fbm potential): advecting along this
// swirls without bunching up or thinning out.
fn curl_noise(p: vec2<f32>) -> vec2<f32> {
    let e = 0.01;
    let dx = fbm(p + vec2<f32>(e, 0.0)) - fbm(p - vec2<f32>(e, 0.0));
    let dy = fbm(p + vec2<f32>(0.0, e)) - fbm(p - vec2<f32>(0.0, e));
    return vec2<f32>(dy, -dx) / (2.0 * e);
}

// ---- Scene kit --------------------------------------------------------------
// Building blocks for representational presets (Han River, Jongno Rain…).
// Distances are in `centered` units (screen heights); negative is inside.

fn sd_circle(p: vec2<f32>, r: f32) -> f32 {
    return length(p) - r;
}

// Axis-aligned box with half-size `b`, centered at the origin.
fn sd_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

// Distance to the segment a–b (a line with round caps; subtract a width).
fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Antialiased solid fill of a signed distance: 1 inside, 0 outside.
fn fill(d: f32) -> f32 {
    let aa = 1.0 / max(u.resolution.y, 1.0);
    return smoothstep(aa, -aa, d);
}

// Soft light falloff around a shape: 1 at its edge, ~0.37 at distance `r`.
fn glow(d: f32, r: f32) -> f32 {
    let x = max(d, 0.0) / r;
    return exp(-x * x);
}

// A mountain ridgeline: height 0..1 along `x`, `seed` picks the range,
// `rough` 0..1 trades rolling hills for jagged peaks.
fn ridge(x: f32, seed: f32, rough: f32) -> f32 {
    let p = vec2<f32>(x, seed * 13.7);
    let broad = fbm(p * 1.3);
    let fine = vnoise(p * 9.0) * 0.5 + vnoise(p * 23.0) * 0.25;
    return clamp(broad * (1.0 - 0.35 * rough) + (fine - 0.375) * 0.4 * rough, 0.0, 1.0);
}

// Profile of a hanok (Korean tiled) roof, `x` in -1..1 across its width:
// a steep concave sweep from the ridge that flattens toward the eaves and
// lifts at the tips. Returns the roof's top height 0..1 (1 at the ridge),
// or -1 outside the roof.
fn hanok_roof(x: f32) -> f32 {
    let a = abs(x);
    if (a > 1.0) {
        return -1.0;
    }
    let lift = smoothstep(0.7, 1.0, a);
    return (1.0 - a) * (1.0 - a) * 0.85 + 0.15 + 0.15 * lift * lift;
}

// A static starfield at `p`: `scale` cells per unit, `density` 0..1 the
// fraction of cells holding a star. Returns brightness 0..1; multiply by
// your own twinkle.
fn stars(p: vec2<f32>, scale: f32, density: f32) -> f32 {
    let q = p * scale;
    let cell = floor(q);
    let pos = vec2<f32>(rand2(cell), rand2(cell + 71.3)) * 0.8 + 0.1;
    let d = length(fract(q) - pos);
    let size = 0.04 + 0.08 * rand2(cell + 19.1);
    return step(rand2(cell + 5.7), density) * smoothstep(size, 0.0, d);
}

// Falling rain streaks at `p` and time `t`: `density` 0..1, `slant` the
// horizontal drift per unit of fall. Returns streak brightness 0..1.
fn rain(p: vec2<f32>, t: f32, density: f32, slant: f32) -> f32 {
    let q = vec2<f32>(p.x - p.y * slant, p.y) * vec2<f32>(160.0, 3.0);
    let col = floor(q.x);
    let speed = 2.5 + 1.5 * rand2(vec2<f32>(col, 1.0));
    let y = fract(q.y + t * speed + rand2(vec2<f32>(col, 2.0)));
    let streak = smoothstep(0.0, 0.08, y) * smoothstep(0.30, 0.10, y);
    let thin = smoothstep(0.5, 0.1, abs(fract(q.x) - 0.5));
    return streak * thin * step(rand2(vec2<f32>(col, 3.0)), density);
}
