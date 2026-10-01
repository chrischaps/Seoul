// Feedback warp: resample last frame's image through a per-pixel UV
// transform, fade it by `decay`, and write it as the base of this frame.
//
// Coordinates here are texture space: uv (0,0) is the top-left texel.
// Every parameter arrives already normalized to "per 1/60 s" by the CPU
// (see `render/warp.rs::WarpParams::to_uniforms`), so trails look the same
// at any refresh rate.

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

// Fetch the previous frame with the preset's edge policy. Mirroring keeps
// zoom-out presets from smearing border pixels inward as streaks.
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

// Rotate hue by `a` radians (Rodrigues rotation about the grey axis).
fn hue_rotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735027);
    let cs = cos(a);
    return c * cs + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cs);
}

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let src = warp_uv(in.uv);
    var col = sample_prev(src);

    if (w.blur != 0.0 || w.sharpen != 0.0) {
        let o = w.texel;
        let avg = (sample_prev(src + vec2<f32>(o.x, 0.0))
                 + sample_prev(src - vec2<f32>(o.x, 0.0))
                 + sample_prev(src + vec2<f32>(0.0, o.y))
                 + sample_prev(src - vec2<f32>(0.0, o.y))) * 0.25;
        col = mix(col, avg, clamp(w.blur, 0.0, 1.0));
        col = col + (col - avg) * w.sharpen;
    }

    if (w.hue_shift != 0.0) {
        col = hue_rotate(col, w.hue_shift);
    }

    return vec4<f32>(max(col, vec3<f32>(0.0)) * w.decay, 1.0);
}
