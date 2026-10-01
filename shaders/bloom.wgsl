// Physically-flavored bloom over a mip chain (Jimenez, "Next Generation Post
// Processing in Call of Duty: Advanced Warfare", SIGGRAPH 2014):
//   prefilter  : feedback → mip 0, soft-knee threshold + Karis average
//   downsample : mip i-1 → mip i, 13-tap filter
//   upsample   : mip i → mip i-1, 9-tap tent, blended additively

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct BloomUniforms {
    // Texel size of `src`.
    texel: vec2<f32>,
    threshold: f32,
    knee: f32,
};
@group(0) @binding(2) var<uniform> b: BloomUniforms;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bloom(@builtin(vertex_index) vid: u32) -> Varying {
    let c = vec2<f32>(f32(vid & 1u) * 2.0, f32((vid >> 1u) & 1u) * 2.0);
    var out: Varying;
    out.pos = vec4<f32>(c * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    return out;
}

fn tap(uv: vec2<f32>, dx: f32, dy: f32) -> vec3<f32> {
    return textureSampleLevel(src, samp, uv + vec2<f32>(dx, dy) * b.texel, 0.0).rgb;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Weight that tames single-pixel fireflies before they smear across mips.
fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luma(c));
}

// Soft-knee threshold: keeps a smooth roll-in instead of a hard cutoff.
fn threshold(c: vec3<f32>) -> vec3<f32> {
    let br = max(max(c.r, c.g), c.b);
    var rq = clamp(br - b.threshold + b.knee, 0.0, 2.0 * b.knee);
    rq = rq * rq / (4.0 * b.knee + 1e-4);
    let contrib = max(rq, br - b.threshold) / max(br, 1e-4);
    return c * contrib;
}

// 13 taps as five overlapping 2×2 boxes.
fn downsample13(uv: vec2<f32>, karis_avg: bool) -> vec3<f32> {
    let a = tap(uv, -2.0, -2.0);
    let bb = tap(uv, 0.0, -2.0);
    let c = tap(uv, 2.0, -2.0);
    let d = tap(uv, -2.0, 0.0);
    let e = tap(uv, 0.0, 0.0);
    let f = tap(uv, 2.0, 0.0);
    let g = tap(uv, -2.0, 2.0);
    let h = tap(uv, 0.0, 2.0);
    let i = tap(uv, 2.0, 2.0);
    let j = tap(uv, -1.0, -1.0);
    let k = tap(uv, 1.0, -1.0);
    let l = tap(uv, -1.0, 1.0);
    let m = tap(uv, 1.0, 1.0);

    let g0 = (j + k + l + m) * 0.25;
    let g1 = (a + bb + d + e) * 0.25;
    let g2 = (bb + c + e + f) * 0.25;
    let g3 = (d + e + g + h) * 0.25;
    let g4 = (e + f + h + i) * 0.25;
    if (karis_avg) {
        let w0 = karis(g0) * 0.5;
        let w1 = karis(g1) * 0.125;
        let w2 = karis(g2) * 0.125;
        let w3 = karis(g3) * 0.125;
        let w4 = karis(g4) * 0.125;
        return (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
    }
    return g0 * 0.5 + (g1 + g2 + g3 + g4) * 0.125;
}

@fragment
fn fs_prefilter(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(threshold(downsample13(in.uv, true)), 1.0);
}

@fragment
fn fs_down(in: Varying) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample13(in.uv, false), 1.0);
}

@fragment
fn fs_up(in: Varying) -> @location(0) vec4<f32> {
    // 3×3 tent.
    let s = tap(in.uv, -1.0, -1.0) + tap(in.uv, 1.0, -1.0)
          + tap(in.uv, -1.0, 1.0) + tap(in.uv, 1.0, 1.0)
          + (tap(in.uv, 0.0, -1.0) + tap(in.uv, -1.0, 0.0)
           + tap(in.uv, 1.0, 0.0) + tap(in.uv, 0.0, 1.0)) * 2.0
          + tap(in.uv, 0.0, 0.0) * 4.0;
    return vec4<f32>(s / 16.0, 1.0);
}
