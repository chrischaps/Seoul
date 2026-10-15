// Feedback Julia warp: sample last frame at z² + c. Iterating this through
// the feedback loop is the Julia iteration itself, one step per frame.

const SCALE: f32 = 2.9; // complex-plane units per screen height

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let z = centered(in.uv) * SCALE;
    let w = csq(z) + vec2<f32>(q.cx, q.cy);
    let src = uncentered(w / SCALE);
    return finish(src, sample_prev(src));
}
