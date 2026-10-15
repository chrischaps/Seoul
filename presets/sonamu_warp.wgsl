// Sonamu warp: last frame drifts gently upward along a slow curl-noise
// current, so the pine's light rises off it like ink mist off wet paper.

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let c = centered(in.uv);
    let t = w.time;
    let flow = curl_noise(c * 1.8 + vec2<f32>(t * 0.02, -t * 0.03));
    let d = (flow * (0.0004 + u.mid * 0.0008) + vec2<f32>(0.0, 0.0004)) * frame_k();

    // Centered coords are y-up; texture space is y-down.
    let src = warp_uv(in.uv) - vec2<f32>(d.x / w.aspect, -d.y);
    let col = sample_prev(src);
    return finish(src, col);
}
