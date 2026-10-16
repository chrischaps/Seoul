// Echo Chamber — five sparks on slow, incommensurate Lissajous orbits. The
// warp's gentle zoom and hue drift stretch their trails into rainbow
// ribbons; the post pass folds the frame into four-way symmetry and lays a
// flipped, zoomed echo behind it, so a few points of light become a hall
// of mirrors.

fn spark(p: vec2<f32>, c: vec2<f32>, r: f32) -> f32 {
    let d = p - c;
    return exp(-dot(d, d) / (r * r));
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p = centered(in.uv);
    // Orbit time runs faster under cv3; the beat is the patch's gate 1
    // once it plays one, and cv1 brightens.
    let t = q.orbit;
    let lift = max(u.cv[0], 0.0);
    var col = vec3<f32>(0.0);
    for (var i = 0u; i < 5u; i = i + 1u) {
        let fi = f32(i);
        let c = vec2<f32>(
            sin(t * (0.41 + fi * 0.13) + fi * 2.1) * 0.36,
            cos(t * (0.53 + fi * 0.07) + fi) * 0.30,
        );
        let r = 0.010 + u.bass * 0.008 + q.pulse * 0.010 + lift * 0.004;
        col = col + palette.colors[i % 4u].rgb * spark(p, c, r) * (0.35 + q.pulse * 0.6 + lift * 0.4);
    }
    return vec4<f32>(col, 1.0);
}
