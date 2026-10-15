// Han River warp — above the horizon, last frame simply lingers (short
// light trails). Below it, each pixel is the sky mirrored across the
// horizon by `water_reflect`: two crossing ripple trains stirred by the
// bass and shivered by each kick, dimming with depth, so every light in the city gets a trembling
// reflection.

const HORIZON_T: f32 = 0.60; // texture space (y down) of the composite's 0.40

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    if (uv.y < HORIZON_T) {
        let src = warp_uv(uv);
        return finish(src, sample_prev(src));
    }
    let refl = water_reflect(uv, HORIZON_T, 0.5 + u.bass * 0.6 + u.kick * 1.6, u.mid * 3.0);
    return vec4<f32>(refl * w.decay, 1.0);
}
