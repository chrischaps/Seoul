// Provided to all preset composite shaders. Do not duplicate these declarations
// in your preset's WGSL file — only write `fs_composite`.

struct AudioFeatures {
    bass: f32,
    mid: f32,
    treble: f32,
    volume: f32,
    beat: f32,
    time: f32,
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

// Sample the analyzed waveform at an angle in radians (typically `atan2(p.y, p.x)`).
fn waveform_at(theta: f32) -> f32 {
    let i = u32(clamp((theta / TAU + 0.5) * 512.0, 0.0, 511.0));
    return u.waveform[i];
}

// Sample the log-spaced spectrum at a normalized position (0 = low, 1 = high).
fn spectrum_at(t: f32) -> f32 {
    let i = u32(clamp(t * 64.0, 0.0, 63.0));
    return u.spectrum[i];
}
