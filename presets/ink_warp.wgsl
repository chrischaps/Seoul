// Ink warp — instead of a rigid zoom/rotate, last frame is advected along a
// slowly evolving curl-noise current, so everything drawn diffuses like
// ink in still water. Mid-range energy stirs the water harder.

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let c = centered(in.uv);
    let t = w.time;

    // Divergence-free flow: swirls without pooling or thinning.
    let flow = curl_noise(c * 2.2 + vec2<f32>(t * 0.03, -t * 0.02));
    let d = flow * (0.0008 + u.mid * 0.0014) * frame_k();

    // Centered coords are y-up; texture space is y-down.
    let src = warp_uv(in.uv) - vec2<f32>(d.x / w.aspect, -d.y);

    // A touch of extra spreading keeps edges soft, like wet paper.
    let col = mix(sample_prev(src), blur4(src, 1.5), 0.2);
    return finish(src, col);
}
