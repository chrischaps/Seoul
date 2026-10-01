@group(0) @binding(0) var prev: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// Layout matches Rust WarpUniforms { decay: f32, _pad: [f32;3] } = 16 bytes.
// We avoid vec3<f32> here because its WGSL alignment (16) would bump the
// struct size to 32 bytes, mismatching the Rust side.
struct WarpUniforms {
    decay: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(1) @binding(0) var<uniform> w: WarpUniforms;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_warp(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
) -> Varying {
    var out: Varying;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let col = textureSample(prev, samp, in.uv);
    return vec4<f32>(col.rgb * w.decay, 1.0);
}
