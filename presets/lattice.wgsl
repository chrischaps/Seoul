// Lattice — orthogonal grid of thin lines. Line thickness pulses with bass,
// cells get per-cell palette assignments that drift with time. Warp on beat
// bends the otherwise-rigid grid into organic shapes.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let grid_scale = 14.0;
    let g = in.uv * grid_scale;
    let cell = floor(g);
    let frac = fract(g) - vec2<f32>(0.5);

    // Thin lines along both axes; thickness swells with bass.
    let thickness = 0.035 + u.bass * 0.050;
    let line = step(min(abs(frac.x), abs(frac.y)), thickness);

    // Per-cell color index drifts — gives the grid a slow color "shimmer".
    let cell_t = fract((cell.x * 0.13 + cell.y * 0.09) + u.time * 0.08);
    let col_a = mix(palette.colors[0].rgb, palette.colors[1].rgb, cell_t);
    let col_b = mix(palette.colors[2].rgb, palette.colors[3].rgb, cell_t);
    let col = mix(col_a, col_b, step(0.5, cell_t));

    // Gentle vignette so edges are dim
    let p = centered(in.uv);
    let vignette = 1.0 - smoothstep(0.35, 0.75, length(p));

    let intensity = line * vignette * (0.025 + u.beat * 0.045);
    return vec4<f32>(col * intensity, 1.0);
}
