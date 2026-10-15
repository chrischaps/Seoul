// Cheonggyecheon Lanterns warp — on the water, last frame drifts
// downstream with the current (faster in the near lanes, as the
// composite's lanterns do) and wavers a little, so lantern light leaves
// soft trails like a long exposure. The banks and the walk stay put.

const WATER_TOP_T: f32 = 0.30; // texture space (y down) of the composite's 0.70
const WATER_BOT_T: f32 = 0.84; // … and of 0.16

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    if (uv.y < WATER_TOP_T || uv.y > WATER_BOT_T) {
        return finish(uv, sample_prev(uv));
    }
    // Lane speed falls off toward the far bank, matching lane_scale().
    let far = clamp((0.75 - uv.y) / (0.75 - 0.38), 0.0, 1.0);
    let speed = params().speed * mix(1.0, 0.62, far);
    let waver = curl_noise(vec2<f32>(uv.x * 4.0, uv.y * 12.0) + vec2<f32>(w.time * 0.1, 0.0));
    let src = uv - (vec2<f32>(speed / 60.0, 0.0) + waver * 0.00005) * frame_k();
    let s = vec2<f32>(src.x, clamp(src.y, WATER_TOP_T, WATER_BOT_T));
    return finish(s, sample_prev(s));
}
