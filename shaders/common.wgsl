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

    spectrum: array<f32, 64>,
    waveform: array<f32, 512>,
};

struct Palette {
    colors: array<vec4<f32>, 4>,
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
