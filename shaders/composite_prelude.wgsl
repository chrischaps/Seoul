// Provided to all preset composite shaders. Do not duplicate these declarations
// in your preset's WGSL file — only write `fs_composite`.

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

@group(0) @binding(0) var<storage, read> u: AudioFeatures;
@group(1) @binding(0) var<uniform> palette: Palette;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

const TAU: f32 = 6.2831853;
const PI:  f32 = 3.1415927;

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> Varying {
    let uv = vec2<f32>(
        f32(vid & 1u) * 2.0,
        f32((vid >> 1u) & 1u) * 2.0,
    );
    var out: Varying;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

// Aspect-corrected coordinates centered on the screen: y spans -0.5..0.5
// (bottom → top) and x spans ±0.5·aspect, so `length(centered(uv))` draws
// true circles at any window shape.
fn centered(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>((uv.x - 0.5) * u.aspect, uv.y - 0.5);
}

// (radius, angle) of `centered(uv)`; angle in -PI..PI.
fn polar(uv: vec2<f32>) -> vec2<f32> {
    let c = centered(uv);
    return vec2<f32>(length(c), atan2(c.y, c.x));
}

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
