// Provided to every warp shader (after shaders/common.wgsl): the built-in
// one (shaders/warp_default.wgsl) and any preset's custom `[shader] warp`
// file. A custom warp file defines only:
//
//     @fragment
//     fn fs_warp(in: Varying) -> @location(0) vec4<f32>
//
// It must return last frame's image, transformed however you like and
// multiplied by `w.decay` — that product is what leaves trails. Coordinates
// here are TEXTURE space: in.uv (0,0) is the top-left, y points DOWN.
// `warp_uv(in.uv)` applies the preset's [mapping] transform (zoom, rotation,
// center, translate, stretch, wobble), so a custom warp can build on it.
//
// Every uniform is already normalized to "per 1/60 s", so warps behave the
// same at any refresh rate.

@group(0) @binding(0) var prev: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// Layout matches Rust `WarpUniforms` (5 × vec4 = 80 bytes).
struct WarpUniforms {
    decay: f32,
    zoom: f32,
    rotation: f32,
    warp_amount: f32,

    cx: f32,
    cy: f32,
    dx: f32,
    dy: f32,

    sx: f32,
    sy: f32,
    warp_scale: f32,
    warp_speed: f32,

    time: f32,
    aspect: f32,
    hue_shift: f32,
    blur: f32,

    sharpen: f32,
    edge_mode: f32,
    texel: vec2<f32>,
};
@group(1) @binding(0) var<uniform> w: WarpUniforms;
@group(2) @binding(0) var<storage, read> u: AudioFeatures;
@group(3) @binding(0) var<uniform> palette: Palette;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_warp(@builtin(vertex_index) vid: u32) -> Varying {
    let c = vec2<f32>(f32(vid & 1u) * 2.0, f32((vid >> 1u) & 1u) * 2.0);
    var out: Varying;
    out.pos = vec4<f32>(c * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    return out;
}

const EDGE_MIRROR: f32 = 0.0;
const EDGE_FADE: f32 = 1.0;

// Last frame at texture-space `uv`, honoring the preset's edge policy.
// Mirroring keeps zoom-out presets from smearing borders inward as streaks.
fn sample_prev(uv: vec2<f32>) -> vec3<f32> {
    if (w.edge_mode == EDGE_MIRROR) {
        let m = 1.0 - abs(fract(uv * 0.5) * 2.0 - 1.0);
        return textureSampleLevel(prev, samp, m, 0.0).rgb;
    }
    let c = textureSampleLevel(prev, samp, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
    if (w.edge_mode == EDGE_FADE) {
        let outside = max(max(-uv, uv - 1.0), vec2<f32>(0.0));
        let fade = 1.0 - smoothstep(0.0, 0.015, max(outside.x, outside.y));
        return c * fade;
    }
    return c;
}

// Average of the four neighbors `r` texels away — for blur/sharpen/edges.
fn blur4(uv: vec2<f32>, r: f32) -> vec3<f32> {
    let o = w.texel * r;
    return (sample_prev(uv + vec2<f32>(o.x, 0.0))
          + sample_prev(uv - vec2<f32>(o.x, 0.0))
          + sample_prev(uv + vec2<f32>(0.0, o.y))
          + sample_prev(uv - vec2<f32>(0.0, o.y))) * 0.25;
}

// Frame-time factor: multiply any per-frame displacement you add yourself
// by this so it moves the same distance per second at any refresh rate.
// (Everything in `w` is already normalized.)
fn frame_k() -> f32 {
    return clamp(u.dt * 60.0, 0.0, 6.0);
}

// Texture-space uv → aspect-correct centered coords with y UP (matching the
// composite prelude's `centered`), and back.
fn centered(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>((uv.x - 0.5) * w.aspect, 0.5 - uv.y);
}
fn uncentered(c: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(c.x / w.aspect + 0.5, 0.5 - c.y);
}

// The preset's [mapping] transform: where this pixel samples last frame.
fn warp_uv(uv: vec2<f32>) -> vec2<f32> {
    // Zoom / stretch / rotate about (cx, cy) in aspect-corrected space so
    // rotation doesn't shear a non-square frame.
    let center = vec2<f32>(w.cx, w.cy);
    var q = uv - center;
    q.x = q.x * w.aspect;
    q = q / (vec2<f32>(w.sx, w.sy) * w.zoom);
    let cs = cos(w.rotation);
    let sn = sin(w.rotation);
    q = vec2<f32>(cs * q.x - sn * q.y, sn * q.x + cs * q.y);
    q.x = q.x / w.aspect;
    var src = q + center - vec2<f32>(w.dx, w.dy);

    // Classic MilkDrop-style sinusoidal wobble.
    let p = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let t = w.time * w.warp_speed;
    let k = 3.0 * w.warp_scale;
    src = src + w.warp_amount * vec2<f32>(sin(p.y * k + t * 0.5), cos(p.x * k + t * 0.7));
    return src;
}

// The built-in finishing steps (blur, sharpen, hue drift, decay) for a color
// already fetched at `src`. Custom warps can call this to stay consistent
// with the [mapping] knobs, or skip it and do their own thing.
fn finish(src: vec2<f32>, col_in: vec3<f32>) -> vec4<f32> {
    var col = col_in;
    if (w.blur != 0.0 || w.sharpen != 0.0) {
        let avg = blur4(src, 1.0);
        col = mix(col, avg, clamp(w.blur, 0.0, 1.0));
        col = col + (col - avg) * w.sharpen;
    }
    if (w.hue_shift != 0.0) {
        col = hue_rotate(col, w.hue_shift);
    }
    return vec4<f32>(max(col, vec3<f32>(0.0)) * w.decay, 1.0);
}
