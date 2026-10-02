// Constellations — a grid of square cells (22 rows) of cells, ~3% of which host a star.
// Each star has a pseudo-random hue + phase, twinkles from treble, and
// flashes on beat. Low decay keeps the field mostly dark rather than trailing.
fn hash2(c: vec2<f32>) -> f32 {
    return fract(sin(dot(c, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    // Square cells at any aspect: 22 rows, as many columns as fit.
    let g = vec2<f32>(in.uv.x * u.aspect, in.uv.y) * 22.0;
    let cell = floor(g);
    let local = fract(g) - vec2<f32>(0.5);

    let h1 = hash2(cell);
    let h2 = hash2(cell + vec2<f32>(7.0, 13.0));

    // Star presence: ~3% of cells
    let star_on = step(0.97, h1);

    // Star can sit slightly off-cell-center (random jitter)
    let jitter = vec2<f32>(h2, fract(h2 * 3.17)) * 0.3 - 0.15;
    let d = length(local - jitter);

    // Sharp bright core with a soft halo
    let core = smoothstep(0.06, 0.0, d);
    let halo = smoothstep(0.16, 0.0, d) * 0.35;

    // Per-star twinkle phase
    let twinkle_phase = u.time * (2.0 + h2 * 4.0) + h1 * TAU;
    let twinkle = 0.4 + 0.6 * (0.5 + 0.5 * sin(twinkle_phase));

    // Color pick: most stars are cool white, some warm, rare beat-flash blue
    let warm = step(0.6, h2);
    let col_base = mix(palette.colors[1].rgb, palette.colors[2].rgb, warm);
    let col = mix(col_base, palette.colors[3].rgb, u.beat * 0.5);

    let brightness =
        star_on * (core + halo) * twinkle * (0.35 + u.treble * 0.4 + u.beat * 0.4);

    // A faint drifting nebula behind the stars, in the palette's night blue.
    let c = centered(in.uv);
    let neb = fbm(c * 1.6 + vec2<f32>(u.time * 0.01, 0.0));
    let nebula = palette.colors[0].rgb * pow(neb, 3.0) * (0.030 + u.bass_att * 0.015);

    return vec4<f32>(col * brightness * 1.3 + nebula, 1.0);
}
