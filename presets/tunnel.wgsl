// Tunnel — bright core at center, concentric ring that shrinks with bass,
// sparse outer glow. Strong zoom in the mapping pulls it all inward over time.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);

    var col = vec3<f32>(0.0);
    // The beat, or a Soba patch's gate 1; cv1 brightens, cv2 turns the colors
    let beat = params().pulse;
    let lift = 1.0 + max(u.cv[0], 0.0);
    let turn = u.cv[1] * PI;

    // Tight core
    let core = smoothstep(0.05, 0.0, r);
    col = col + core * palette.colors[3].rgb * (0.12 + beat * 0.25) * lift;

    // Inner ring, radius shrinks as bass rises → "sucked into tunnel"
    let ring_r = 0.18 - u.bass * 0.08;
    let ring = smoothstep(0.010, 0.0, abs(r - ring_r));
    col = col + ring * max(hue_rotate(palette.colors[1].rgb, turn), vec3<f32>(0.0)) * 0.22 * lift;

    // Magenta accent on beat
    let accent_r = 0.28 + u.mid * 0.05;
    let accent = smoothstep(0.012, 0.0, abs(r - accent_r));
    col = col + accent * max(hue_rotate(palette.colors[2].rgb, turn), vec3<f32>(0.0)) * beat * 0.35 * lift;

    // Very soft outer glow
    let outer = smoothstep(0.45, 0.30, r);
    col = col + outer * palette.colors[0].rgb * 0.012;

    return vec4<f32>(col, 1.0);
}
