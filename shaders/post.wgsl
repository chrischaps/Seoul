// Final display pass: HDR feedback + bloom → tonemapped, graded frame.
//
// Everything up to `tonemap` works in scene-linear HDR, so bloom and the LED
// mask can carry energy above 1.0. Vignette, grain and dither then work in
// the display-encoded domain, and the result is linearized once at the end
// for the sRGB swapchain.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

// Layout matches Rust `PostUniforms` (4 × vec4 = 64 bytes).
struct PostUniforms {
    exposure: f32,
    bloom: f32,
    chroma: f32,
    vignette: f32,

    grain: f32,
    led_mask: f32,
    led_pitch: f32,
    saturation: f32,

    time: f32,
    beat: f32,
    resolution: vec2<f32>,

    contrast: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(1) @binding(0) var<uniform> p: PostUniforms;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) vid: u32) -> Varying {
    let c = vec2<f32>(f32(vid & 1u) * 2.0, f32((vid >> 1u) & 1u) * 2.0);
    var out: Varying;
    out.pos = vec4<f32>(c * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    return out;
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn hdr_at(uv: vec2<f32>) -> vec3<f32> {
    let s = textureSampleLevel(scene, samp, uv, 0.0).rgb;
    let b = textureSampleLevel(bloom_tex, samp, uv, 0.0).rgb;
    return s + b * p.bloom;
}

// Khronos PBR Neutral: identity below ~0.76, then a smooth highlight
// shoulder with minimal hue shift. Presets keep the colors they were
// authored with; only what would have clipped gets compressed.
fn tonemap(color_in: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desat = 0.15;
    var c = color_in;
    let x = min(c.r, min(c.g, c.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    c = c - offset;
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c = c * (new_peak / peak);
    let g = 1.0 - 1.0 / (desat * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

// Integer hash (PCG3D, Jarzynski & Olano 2020): no visible lattice, unlike
// the fract(sin(...)) family on integer pixel coordinates.
fn pcg3d(v_in: vec3<u32>) -> vec3<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x = v.x + v.y * v.z;
    v.y = v.y + v.z * v.x;
    v.z = v.z + v.x * v.y;
    return v;
}

fn noise2(frag: vec2<f32>) -> vec2<f32> {
    let h = pcg3d(vec3<u32>(u32(frag.x), u32(frag.y), u32(p.time * 60.0)));
    return vec2<f32>(h.xy) / 4294967295.0;
}

// LED panel: the frame is resampled once per LED cell and drawn as a round
// emitter with a soft halo; unlit LEDs stay faintly visible, like the real
// panel photographed in ref/.
fn led_panel(frag: vec2<f32>) -> vec3<f32> {
    let pitch = max(p.led_pitch * p.resolution.y / 1080.0, 3.0);
    let cell = floor(frag / pitch);
    let center = (cell + 0.5) * pitch / p.resolution;
    let lit = hdr_at(center);

    let local = fract(frag / pitch) - 0.5;
    let r = length(local);
    let core = smoothstep(0.40, 0.26, r);
    let halo = exp(-r * r * 14.0) * 0.35;
    let unlit = vec3<f32>(0.010, 0.011, 0.013) * core;
    return lit * (core * 1.5 + halo) + unlit;
}

@fragment
fn fs_post(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let d = uv - 0.5;

    var col: vec3<f32>;
    if (p.led_mask > 0.5) {
        col = led_panel(in.pos.xy);
    } else if (p.chroma > 0.0) {
        // Radial chromatic aberration, kicked wider on beats.
        let ca = d * p.chroma * (0.004 + 0.010 * p.beat);
        col = vec3<f32>(hdr_at(uv - ca).r, hdr_at(uv).g, hdr_at(uv + ca).b);
    } else {
        col = hdr_at(uv);
    }

    // Grade in linear light, then tonemap.
    col = max(col * p.exposure, vec3<f32>(0.0));
    col = pow(col, vec3<f32>(p.contrast));
    let l = dot(col, LUMA);
    col = max(l + p.saturation * (col - l), vec3<f32>(0.0));
    var s = linear_to_srgb(tonemap(col));

    // Vignette in aspect-correct space.
    let aspect = p.resolution.x / max(p.resolution.y, 1.0);
    let vd = length(vec2<f32>(d.x * aspect, d.y)) / length(vec2<f32>(0.5 * aspect, 0.5));
    s = s * mix(1.0, 1.0 - smoothstep(0.45, 1.05, vd), p.vignette);

    // Grain and dither in the encoded domain: in linear light the sRGB
    // curve would amplify them ~13× near black.
    let n = noise2(in.pos.xy);
    let lum = dot(s, LUMA);
    s = s + (n.x - 0.5) * p.grain * 0.10 * sqrt(lum);
    // ±1 LSB triangular dither breaks up banding in long dark gradients.
    s = s + (n.x + n.y - 1.0) / 255.0;

    return vec4<f32>(srgb_to_linear(clamp(s, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
