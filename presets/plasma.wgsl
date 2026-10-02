// Plasma — three interfering sinusoidal fields, audio-modulated, summed into
// one height field. Instead of painting the field (which averages palette
// colors into mud), only narrow contour bands of it light up, each colored
// by a slow cycle through the palette; the heavy warp in the mapping then
// smears those bands into flowing psychedelic ribbons.
//
// Brightness note: composite output is added every frame on top of the
// decayed feedback, so steady state is roughly C / (1 - decay) ≈ 33×C at
// decay 0.97. Keep per-frame contributions small.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv) * 2.0; // y in -1..1, x scaled by aspect
    let r = length(p);
    let t = u.time;

    let f0 = sin(p.x * 6.0 + t * 0.7);
    let f1 = cos(p.y * 5.0 + t * 0.5 + u.bass * 2.0);
    let f2 = sin((p.x + p.y) * 4.0 + t * 0.3 + u.mid * 3.0);
    let v = f0 + f1 + f2; // -3..3

    // Thin bright contours of the field; beats widen them.
    let band = pow(0.5 + 0.5 * sin(v * 2.4 - t * 0.8), 10.0 - 5.0 * u.beat);
    // Magenta ↔ electric blue; gold flashes on beats (blue + gold would
    // average to olive, so they never share a band); mint stays in the core.
    let hue = (0.5 + 0.5 * sin(v * 0.6 + t * 0.15)) * 0.34;
    var col = palette_ramp(hue) * band * 0.026;
    col = col + palette.colors[2].rgb * band * u.beat * 0.030;

    // Sparse bright center that swells with bass
    let core = smoothstep(0.20 + u.bass * 0.10, 0.0, r);
    col = col + core * palette.colors[3].rgb * 0.010;

    return vec4<f32>(col, 1.0);
}
