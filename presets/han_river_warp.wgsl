// Han River warp — above the horizon, last frame simply lingers (short
// light trails). Below it, each pixel is the sky mirrored across the
// horizon, displaced by two crossing ripple trains and dimming with depth,
// so every light in the city gets a trembling reflection.

const HORIZON_T: f32 = 0.60; // texture space (y down) of the composite's 0.40

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    if (uv.y < HORIZON_T) {
        let src = warp_uv(uv);
        return finish(src, sample_prev(src));
    }

    let depth = uv.y - HORIZON_T;
    let t = w.time;
    let ripple = 0.6 * sin(uv.y * 150.0 - t * 2.2 + u.mid * 3.0)
               + 0.4 * sin(uv.y * 63.0 + uv.x * 5.0 + t * 1.3);
    let stir = 0.6 + u.bass * 0.8;
    var src = vec2<f32>(uv.x, HORIZON_T - depth);
    src.x = src.x + ripple * (0.0015 + depth * 0.03) * stir;
    src.y = src.y - abs(ripple) * depth * 0.02;

    // Water reflects less of the sky the deeper you look.
    let refl = sample_prev(src) * (0.6 - depth * 0.9);
    return vec4<f32>(max(refl, vec3<f32>(0.0)) * w.decay, 1.0);
}
