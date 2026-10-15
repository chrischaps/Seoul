// Indra's Pearls: a reflection group in the Poincaré disk. Each pixel is
// reflected back into the central tile by inverting in whichever mirror
// circle contains it; the number of reflections is its generation, and the
// product of the inversions' stretch factors turns the final distance to a
// mirror into a true screen-space distance for crisp, even rims.

const MIRRORS: i32 = 5;
const MAX_STEPS: i32 = 28;

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    // Dive toward a point on the rim, where the pearls crowd smallest.
    let zoom = exp(-q.dive);
    let view = 2.15 * zoom;
    let rim_point = vec2<f32>(cos(q.aim), sin(q.aim));
    let p = rim_point * (1.0 - zoom) * 0.97 + centered(in.uv) * view;
    let px = view / u.resolution.y;

    // Glide: a Möbius move of the disk (an isometry of the hyperbolic plane).
    let a = vec2<f32>(q.ax, q.ay);
    var z = disk_move(p, a);
    let inside = smoothstep(1.02, 0.98, length(z));
    // Its stretch at p, |f'(p)| = (1 − |a|²) / |1 + ā·p|², so rims keep a
    // constant pixel width.
    let den = vec2<f32>(1.0, 0.0) + cmul(conj(a), p);
    var stretch = (1.0 - dot(a, a)) / max(dot(den, den), 1e-6);

    let r = tan(PI / f32(MIRRORS)) * q.open;
    let dist = sqrt(1.0 + r * r); // orthogonal to the unit circle

    var generation = 0;
    for (var step = 0; step < MAX_STEPS; step = step + 1) {
        var flipped = false;
        for (var k = 0; k < MIRRORS; k = k + 1) {
            let ang = q.spin + f32(k) * TAU / f32(MIRRORS);
            let c = dist * vec2<f32>(cos(ang), sin(ang));
            let d = z - c;
            let d2 = dot(d, d);
            if (d2 < r * r) {
                let s = r * r / d2;
                z = c + d * s;
                stretch = stretch * s;
                generation = generation + 1;
                flipped = true;
            }
        }
        if (!flipped) {
            break;
        }
    }

    // Distance to the nearest mirror, back in screen units.
    var rim = abs(length(z) - 1.0);
    for (var k = 0; k < MIRRORS; k = k + 1) {
        let ang = q.spin + f32(k) * TAU / f32(MIRRORS);
        let c = dist * vec2<f32>(cos(ang), sin(ang));
        rim = min(rim, abs(length(z - c) - r));
    }
    let d_screen = rim / max(stretch, 1e-6);
    let line = smoothstep(px * 1.6, 0.0, d_screen);

    let g = f32(generation);

    // Tiles: alternating celadon and gold by parity, deepening with generation,
    // each generation lit by its own spectrum band.
    let parity = f32(generation % 2);
    let band = spectrum_at(clamp(g / 14.0, 0.0, 1.0));
    let tile = mix(palette.colors[1].rgb, palette.colors[2].rgb, parity);
    var col = tile * (0.001 + band * 0.008) * (0.3 + 0.7 * q.groove) * (1.0 / (1.0 + g * 0.15));

    // The central tile breathes with the kick.
    if (generation == 0) {
        col = col + palette.colors[2].rgb * q.punch * 0.03 * inside;
    }

    // A snare sends a wave of gold out through the generations.
    let wave = exp(-pow(g - u.snare_age * 18.0, 2.0) * 0.5) * exp(-u.snare_age * 3.0);
    col = col + palette.colors[2].rgb * wave * 0.02 * inside;

    // Pearl rims, flaring on the kick.
    col = col + palette.colors[3].rgb * line * (0.012 + q.groove * 0.035 + q.shine * 0.05 + q.punch * 0.08);

    // Outside the disk is the same net seen in reflection: keep it quiet.
    col = col * mix(0.15, 1.0, inside);
    return vec4<f32>(col, 1.0);
}
