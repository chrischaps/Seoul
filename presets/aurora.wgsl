// Aurora — vertical curtains of color that drift sideways, intensity
// fades toward the "horizon" so the bottom stays darker.
// Slow, calm, cool palette. Treble adds a subtle shimmer overhead.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = u.time;

    // Two overlapping curtains at different scales, drifting at different speeds.
    let c1 = sin(uv.x * 5.0 + t * 0.18) * 0.5 + 0.5;
    let c2 = sin(uv.x * 11.0 - t * 0.11 + u.mid * 1.5) * 0.5 + 0.5;
    let curtain = c1 * c2;

    // Vertical envelope: brightest in upper third, fades to black at the horizon.
    let vfade = smoothstep(0.10, 0.55, uv.y) * (1.0 - smoothstep(0.90, 1.05, uv.y));

    // Color blends between two palette stops along Y.
    let col_base = mix(palette.colors[0].rgb, palette.colors[1].rgb, uv.y);
    // Violet shimmer from palette[2] keyed to treble
    let shimmer = palette.colors[2].rgb * u.treble * 0.4;
    // Ice-white crown on beats
    let crown = palette.colors[3].rgb * u.beat * smoothstep(0.50, 0.95, uv.y) * 0.5;

    let col = col_base * curtain + shimmer + crown;
    let intensity = vfade * (0.018 + u.bass * 0.015);
    return vec4<f32>(col * intensity, 1.0);
}
