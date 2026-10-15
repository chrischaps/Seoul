// Han River — Seoul seen from the riverbank at night. Above the horizon:
// a moon, a skyline whose towers rise with the spectrum (bass on the left,
// treble on the right) and twinkle with lit windows, and Banpo Bridge, its
// deck lights strung along a shallow arch. On every kick the bridge's
// Moonlight Rainbow Fountain pours a curtain of color toward the water and
// a light runs out along the deck; snares burst fireworks over the river.
// When the kicks stop (`groove` falls), the fountain stops and the city's
// windows go out, until the drop lights everything at once.
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
    let groove = params().groove;
    // City glow: a warm haze of light pollution hugging the horizon.
    let haze = exp(-(p.y - HORIZON) * 9.0);
    var col = mix(palette.colors[0].rgb, palette.colors[1].rgb, 0.25 * haze * groove) * haze * (0.006 + 0.008 * groove);

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
    // Fewer windows stay lit as the groove fades.
    let lit = step(0.62 + (1.0 - groove) * 0.34, rand2(wcell + vec2<f32>(0.0, floor(u.time * 0.2))));
    let wdot = smoothstep(0.45, 0.2, length(fract(win) - 0.5));
    let flicker = 0.7 + 0.3 * sin(u.time * 3.0 + rand2(wcell) * TAU) * u.treble
        + u.hat * step(0.8, rand2(wcell + vec2<f32>(u.hat_count, 5.0))) * 1.5;
    let warm = mix(palette.colors[1].rgb, palette.colors[3].rgb, rand2(wcell + 9.0) * 0.6);
    col = col + warm * in_tower * lit * wdot * flicker * 0.06;
    col = col + palette.colors[0].rgb * in_tower * 0.010;

    // Banpo Bridge: deck lights along the arch.
    let lamp_x = (floor(p.x * 56.0) + 0.5) / 56.0;
    let lamp = vec2<f32>((lamp_x - 0.5) * u.aspect, deck_y(lamp_x) - 0.5);
    let ld = length(c - lamp);
    // Each kick sends a pulse of light from the middle of the bridge out
    // to both banks.
    let run = exp(-pow((abs(lamp_x - 0.5) - u.kick_age * 1.4) * 14.0, 2.0)) * exp(-u.kick_age * 2.0);
    col = col + palette.colors[1].rgb * (smoothstep(0.004, 0.0015, ld) * (0.12 + run * 0.4) + exp(-ld * ld * 9000.0) * (0.02 + run * 0.12));
    col = col + palette.colors[0].rgb * 2.5 * smoothstep(0.003, 0.0, abs(p.y - dy)) * 0.02;

    // The Moonlight Rainbow Fountain: falling streaks under the deck,
    // hue running along the bridge, surging on every kick and stilled in
    // a breakdown.
    if (p.y < dy && p.x > 0.06 && p.x < 0.94) {
        let fall = (dy - p.y) / (dy - HORIZON);
        let streak = vnoise(vec2<f32>(p.x * 220.0, p.y * 14.0 + u.time * 6.0));
        let hue = hue_rotate(vec3<f32>(1.0, 0.25, 0.3), p.x * TAU * 1.5 + u.time * 0.4);
        let pour = (0.03 + groove * 0.12 + u.kick * 1.4) * (1.0 - fall * 0.7);
        col = col + max(hue, vec3<f32>(0.0)) * pow(streak, 5.0) * pour * 0.13;
    }

    // Fireworks: most snares burst a shell high over the river, a ring of
    // sparks that flies out, sags under gravity and burns out.
    let fw = u.snare_count;
    if (rand2(vec2<f32>(fw, 1.0)) < 0.7 && u.snare_age < 1.4) {
        let age = u.snare_age;
        let at = vec2<f32>((rand2(vec2<f32>(fw, 2.0)) - 0.5) * u.aspect * 0.8, 0.16 + 0.2 * rand2(vec2<f32>(fw, 3.0)));
        let size = 0.08 + 0.06 * rand2(vec2<f32>(fw, 4.0));
        let reach = size * (1.0 - exp(-age * 5.0));
        let hue = palette_ramp(rand2(vec2<f32>(fw, 5.0)) * 0.999) * 0.7 + palette.colors[3].rgb * 0.3;
        var sparks = 0.0;
        for (var k = 0; k < 28; k = k + 1) {
            let fk = f32(k);
            let ang = (fk + rand2(vec2<f32>(fw, 10.0 + fk)) * 0.6) / 28.0 * TAU;
            let speed = 0.75 + 0.25 * rand2(vec2<f32>(fw, 50.0 + fk));
            let sp = at + vec2<f32>(cos(ang), sin(ang)) * reach * speed - vec2<f32>(0.0, age * age * 0.04);
            let tail = at + vec2<f32>(cos(ang), sin(ang)) * reach * speed * 0.8 - vec2<f32>(0.0, age * age * 0.03);
            let d = sd_segment(c, tail, sp);
            sparks = sparks + exp(-d * d / (0.0016 * 0.0016)) + glow(length(c - sp), 0.006) * 0.15;
        }
        let fade = exp(-age * 2.4) * (0.5 + 0.5 * step(0.5, fract(age * 14.0 + rand2(vec2<f32>(fw, 6.0)))));
        col = col + hue * sparks * fade * 0.25 + hue * glow(length(c - at), 0.1) * exp(-age * 9.0) * 0.03;
    }

    return vec4<f32>(col, 1.0);
}
