// Fireflies — almost everything here is the particle layer ([particles] in
// the TOML): a sparse swarm advected by curl noise, each glowing up and
// fading over its life. This shader only lays a low ground mist across the
// bottom of the frame so the field has somewhere to be.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let c = centered(in.uv);
    let ground = smoothstep(0.05, -0.45, c.y);
    let mist = fbm(vec2<f32>(c.x * 2.0 + u.time * 0.02, c.y * 5.0));
    // Soba link: cv2 warms the mist toward amber; cv1 thickens it.
    let tone = mix(mix(palette.colors[0].rgb, palette.colors[1].rgb, 0.08), palette.colors[2].rgb * 0.3, clamp(u.cv[1], 0.0, 1.0));
    let col = tone * ground * mist * (0.010 + u.bass_att * 0.006 + max(u.cv[0], 0.0) * 0.008);
    return vec4<f32>(col, 1.0);
}
