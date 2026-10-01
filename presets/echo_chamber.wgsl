// Echo Chamber — three sparks on slow, incommensurate Lissajous orbits. The
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
    let p = centered(in.uv);
    let t = u.time;
    var col = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let fi = f32(i);
        let c = vec2<f32>(
            sin(t * (0.41 + fi * 0.13) + fi * 2.1) * 0.36,
            cos(t * (0.53 + fi * 0.07) + fi) * 0.30,
        );
        let r = 0.007 + u.bass * 0.008 + u.beat * 0.010;
        col = col + palette.colors[i].rgb * spark(p, c, r) * (0.30 + u.beat * 0.6);
    }
    return vec4<f32>(col, 1.0);
}
