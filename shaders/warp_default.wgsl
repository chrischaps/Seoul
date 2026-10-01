// The built-in warp (used when a preset has no `[shader] warp` file):
// resample last frame through the [mapping] transform, then blur/sharpen,
// hue drift and decay. Prepended with common.wgsl + warp_prelude.wgsl.

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let src = warp_uv(in.uv);
    return finish(src, sample_prev(src));
}
