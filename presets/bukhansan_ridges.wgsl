// Bukhansan Ridges — the granite ranges north of Seoul, layer behind
// layer. Every ridgeline is the spectrum as it sounded a moment ago: the
// nearest is now, each farther one LAG seconds older and narrower, so a
// phrase travels back into the haze as you watch. A day passes at the pace
// of the music (`params().day`): afterglow, then night with stars, a moon
// and the odd snare-struck meteor, then dawn again. Mist pools in the
// valleys and surges with each kick; each snare runs a wave of light back
// across the crests. When the kicks stop (`groove` falls), cloud shadow
// dims the range until the drop.

const LAYERS: i32 = 6;
const LAG: f32 = 1.35; // seconds of history between neighboring ridges

// Height (uv, y up) of ridge `i` (0 = nearest) at screen x.
fn ridge_top(x: f32, i: f32) -> f32 {
    // Farther ridges show the same spectrum shrunk toward the center.
    let t = clamp(0.5 + (x - 0.5) * (1.0 + i * 0.16), 0.0, 1.0);
    let age = i * LAG;
    let s = spectrum_ago(t, age) * 0.5
        + spectrum_ago(t + 0.025, age) * 0.25
        + spectrum_ago(t - 0.025, age) * 0.25;
    let rock = ridge(x * (1.8 + i * 0.35) + i * 3.1, i + 1.0, 0.7);
    let base = 0.14 + i * 0.075;
    let amp = 0.22 - i * 0.02;
    return base + (rock * 0.4 + s * 0.8) * amp;
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    let c = centered(p);
    let t = u.time;
    let q = params();
    // The drums, or a Soba patch playing them; cv1 lifts the cloud shadow
    let kick = drum(KICK);
    let snare = drum(SNARE);
    let hat = drum(HAT);
    let groove = min(q.groove + max(u.cv[0], 0.0), 1.0);
    // 1 at dusk/dawn afterglow, 0 in the middle of the night. cv2 turns
    // the hour forward or back.
    let light = 0.5 + 0.5 * cos((q.day + u.cv[1] * 0.25) * TAU);
    let night = 1.0 - light;
    // Cloud shadow in a breakdown.
    let shade = 0.5 + 0.5 * groove;

    // Sky: afterglow pooling over the ridges under a deepening blue.
    let warm = palette.colors[1].rgb;
    let top = mix(vec3<f32>(0.003, 0.005, 0.016), palette.colors[0].rgb * 0.07, light);
    let horizon = mix(vec3<f32>(0.016, 0.024, 0.05), warm * 0.32, light);
    // Distant rock goes blue (atmospheric perspective), warmed a little by the glow.
    let haze_col = mix(palette.colors[2].rgb * mix(0.025, 0.11, light), horizon, 0.12);
    let glow_band = exp(-max(p.y - 0.52, 0.0) * 5.0);
    var col = mix(top, horizon, glow_band) * shade;

    // Stars come out as it darkens; the treble sets them twinkling.
    let sp = c + vec2<f32>(t * 0.002, 0.0);
    let star = stars(sp, 70.0, 0.35);
    let twinkle = 0.6 + 0.4 * sin(t * (2.0 + 6.0 * u.treble) + rand2(floor(sp * 70.0)) * TAU)
        + hat.hit * step(0.7, rand2(floor(sp * 70.0) + vec2<f32>(hat.count, 0.0))) * 1.2;
    col = col + palette.colors[3].rgb * star * twinkle * night * night * 0.5 * smoothstep(0.3, 0.6, p.y);

    // A crescent moon, high on the right.
    let mc = c - vec2<f32>(0.32 * u.aspect, 0.30);
    let crescent = fill(sd_circle(mc, 0.038)) * (1.0 - fill(sd_circle(mc - vec2<f32>(0.014, 0.008), 0.034)));
    col = col + palette.colors[3].rgb * (crescent * 0.9 + glow(length(mc) - 0.038, 0.06) * 0.03) * night;

    // Now and then a snare throws a meteor across the night sky.
    let m = snare.count;
    if (rand2(vec2<f32>(m, 3.0)) < 0.3 && snare.age < 0.7 && night > 0.3) {
        let start = vec2<f32>((rand2(vec2<f32>(m, 4.0)) - 0.3) * u.aspect * 0.8, 0.35 + 0.12 * rand2(vec2<f32>(m, 5.0)));
        let dir = normalize(vec2<f32>(-1.0, -0.45));
        let head = start + dir * snare.age * 0.9;
        let d = sd_segment(c, head - dir * 0.14, head);
        let along = clamp(dot(c - (head - dir * 0.14), dir) / 0.14, 0.0, 1.0);
        let fade = (1.0 - snare.age / 0.7) * night;
        col = col + palette.colors[3].rgb * smoothstep(0.0025, 0.0, d) * along * along * fade * 1.5;
    }

    // Ridges, farthest first: each is darker at its crest and fades
    // downward into the mist pooled in the valley in front of the next.
    let mist_amt = (0.3 + u.bass_att * 0.15 + kick.hit * 0.55) * (0.6 + 0.4 * groove);
    // Lit mist: on a kick it glows with whatever light is in the sky.
    let mist_glow = mix(palette.colors[3].rgb * 0.05, palette.colors[1].rgb * 0.09, light) * kick.hit;
    for (var k = LAYERS - 1; k >= 0; k = k - 1) {
        let i = f32(k);
        let h = ridge_top(p.x, i);
        if (p.y < h) {
            let atm = i / f32(LAYERS - 1);
            let rock = mix(palette.colors[0].rgb * 0.003, haze_col * 0.9, pow(atm, 0.8) * 0.85) * shade;
            let mist = smoothstep(0.0, 0.10, h - p.y) * mist_amt * (0.1 + 0.7 * atm);
            col = mix(rock, haze_col * shade + mist_glow, clamp(mist, 0.0, 1.0));
            // The afterglow catches the crests; a snare's light runs back
            // through the ranges, reaching ridge i after i × 70 ms.
            let crest = smoothstep(0.006, 0.0, h - p.y);
            let wave = exp(-pow((snare.age - i * 0.07) * 18.0, 2.0)) * exp(-snare.age * 1.5);
            let crest_light = mix(palette.colors[3].rgb, warm, light);
            col = col + warm * crest * light * (0.07 - atm * 0.05) * shade
                + crest_light * crest * wave * (0.25 - atm * 0.12)
                + crest_light * smoothstep(0.03, 0.0, h - p.y) * wave * 0.03;
        }
    }

    // Decay is 0.5, so feedback doubles a steady image: halve to land on `col`.
    return vec4<f32>(col * 0.5, 1.0);
}
