// Tunnel — bright core at center, concentric ring that shrinks with bass,
// sparse outer glow. Strong zoom in the mapping pulls it all inward over time.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv - vec2<f32>(0.5);
    let r = length(p);

    var col = vec3<f32>(0.0);

    // Tight core
    let core = smoothstep(0.05, 0.0, r);
    col = col + core * palette.colors[3].rgb * (0.35 + u.beat * 0.55);

    // Inner ring, radius shrinks as bass rises → "sucked into tunnel"
    let ring_r = 0.18 - u.bass * 0.08;
    let ring = smoothstep(0.010, 0.0, abs(r - ring_r));
    col = col + ring * palette.colors[1].rgb * 0.7;

    // Magenta accent on beat
    let accent_r = 0.28 + u.mid * 0.05;
    let accent = smoothstep(0.012, 0.0, abs(r - accent_r));
    col = col + accent * palette.colors[2].rgb * u.beat * 0.8;

    // Very soft outer glow
    let outer = smoothstep(0.45, 0.30, r);
    col = col + outer * palette.colors[0].rgb * 0.15;

    return vec4<f32>(col, 1.0);
}
