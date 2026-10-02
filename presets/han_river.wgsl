// Han River — Seoul seen from the riverbank at night. Above the horizon:
// a moon, a skyline whose towers rise with the spectrum (bass on the left,
// treble on the right) and twinkle with lit windows, and Banpo Bridge, its
// deck lights strung along a shallow arch. On every beat the bridge's
// Moonlight Rainbow Fountain pours a curtain of color toward the water.
// Nothing is drawn below the horizon: the custom warp (han_river_warp.wgsl)
// reflects the sky into the river with a living ripple.

const HORIZON: f32 = 0.40;

fn deck_y(x: f32) -> f32 {
    return HORIZON + 0.075 + 0.02 * sin(PI * x);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    if (p.y < HORIZON) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let c = centered(p);
    // City glow: a warm haze of light pollution hugging the horizon.
    let haze = exp(-(p.y - HORIZON) * 9.0);
    var col = mix(palette.colors[0].rgb, palette.colors[1].rgb, 0.25 * haze) * haze * 0.012;

    // Moon, high on the right, breathing slightly with the bass.
    let moon_c = vec2<f32>(0.30 * u.aspect, 0.30);
    let md = length(c - moon_c);
    let moon_r = 0.045 + u.bass_att * 0.004;
    col = col + palette.colors[3].rgb * (smoothstep(moon_r, moon_r - 0.003, md) * 0.08
                                        + exp(-md * md * 90.0) * 0.006);

    // Skyline: 48 towers rising behind the bridge (which hides their feet),
    // each lit window a small warm square.
    let dy = deck_y(p.x);
    let cols = 48.0;
    let tower = floor(p.x * cols);
    let tx = (tower + 0.5) / cols;
    let jitter = 0.55 + 0.45 * rand2(vec2<f32>(tower, 3.0));
    let top = HORIZON + (0.13 + spectrum_at(tx) * 0.22) * jitter;
    let in_tower = step(p.y, top) * step(dy + 0.006, p.y)
        * step(0.12, fract(p.x * cols)) * step(fract(p.x * cols), 0.88);
    let win = vec2<f32>(c.x, p.y - HORIZON) * 150.0;
    let wcell = floor(win);
    let lit = step(0.62, rand2(wcell + vec2<f32>(0.0, floor(u.time * 0.2))));
    let wdot = smoothstep(0.45, 0.2, length(fract(win) - 0.5));
    let flicker = 0.7 + 0.3 * sin(u.time * 3.0 + rand2(wcell) * TAU) * u.treble;
    let warm = mix(palette.colors[1].rgb, palette.colors[3].rgb, rand2(wcell + 9.0) * 0.6);
    col = col + warm * in_tower * lit * wdot * flicker * 0.06;
    col = col + palette.colors[0].rgb * in_tower * 0.010;

    // Banpo Bridge: deck lights along the arch.
    let lamp_x = (floor(p.x * 56.0) + 0.5) / 56.0;
    let lamp = vec2<f32>((lamp_x - 0.5) * u.aspect, deck_y(lamp_x) - 0.5);
    let ld = length(c - lamp);
    col = col + palette.colors[1].rgb * (smoothstep(0.004, 0.0015, ld) * 0.12 + exp(-ld * ld * 9000.0) * 0.02);
    col = col + palette.colors[0].rgb * 2.5 * smoothstep(0.003, 0.0, abs(p.y - dy)) * 0.02;

    // The Moonlight Rainbow Fountain: falling streaks under the deck,
    // hue running along the bridge, pouring hardest on the beat.
    if (p.y < dy && p.x > 0.06 && p.x < 0.94) {
        let fall = (dy - p.y) / (dy - HORIZON);
        let streak = vnoise(vec2<f32>(p.x * 220.0, p.y * 14.0 + u.time * 6.0));
        let hue = hue_rotate(vec3<f32>(1.0, 0.25, 0.3), p.x * TAU * 1.5 + u.time * 0.4);
        let pour = (0.08 + u.beat * 0.92) * (1.0 - fall * 0.7);
        col = col + max(hue, vec3<f32>(0.0)) * pow(streak, 6.0) * pour * 0.12;
    }

    return vec4<f32>(col, 1.0);
}
