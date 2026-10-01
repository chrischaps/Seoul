// Provided to all preset composite shaders (after shaders/common.wgsl).
// Do not duplicate these declarations in your preset's WGSL file — only
// write `fs_composite`.
//
// `in.uv` runs 0..1 with y pointing UP (0 = bottom of the screen).

@group(0) @binding(0) var<storage, read> u: AudioFeatures;
@group(1) @binding(0) var<uniform> palette: Palette;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

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
