// Jeongdongjin Sunrise — dawn on the east coast, where Seoul goes to watch
// the sun come out of the sea. Each wave crest is born at the horizon and
// marches toward the shore in perspective, carrying the spectrum from the
// moment it was born (spectrum_ago), so the song washes in from the sea.
// The sun's elevation follows how much music has played (`params().sun`):
// it climbs out of the water, crosses the sky and sets again. Hats glint
// on the sun's path across the water, kicks flash foam on the nearest
// crests, and the ship-shaped hotel keeps watch from the headland.

const HORIZON: f32 = 0.42;
const WAVES: i32 = 26;
const WAVE_SPACING: f32 = 0.32; // seconds of history between crests
const Z_FAR: f32 = 9.0;         // ground distance at the horizon end
const Z_NEAR: f32 = 1.0;        // … and at the bottom of the frame

// Screen depth below the horizon (0 at the horizon, 1 at the bottom) of a
// crest that is `age` seconds old: crests travel at constant ground speed,
// so they bunch up toward the horizon and spread out near the shore.
fn wave_v(age: f32) -> f32 {
    let z = mix(Z_FAR, Z_NEAR, clamp(age / (f32(WAVES) * WAVE_SPACING), 0.0, 1.0));
    return Z_NEAR / z;
}

// The ship-shaped hotel on the cliff: hull, two decks and a bridge,
// in cliff-local coords (origin at the hull's waterline center).
fn sd_ship(q: vec2<f32>) -> f32 {
    // Hull: a band whose bow (left) rakes forward.
    let rake = max(-q.x - 0.05, 0.0) * 0.6;
    var d = sd_box(q - vec2<f32>(0.0, 0.012 + rake * 0.5), vec2<f32>(0.075 - rake, 0.012));
    d = min(d, sd_box(q - vec2<f32>(0.01, 0.032), vec2<f32>(0.05, 0.008)));
    d = min(d, sd_box(q - vec2<f32>(0.02, 0.046), vec2<f32>(0.028, 0.006)));
    d = min(d, sd_box(q - vec2<f32>(0.03, 0.058), vec2<f32>(0.010, 0.006)));
    return d;
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    let c = centered(p);
    let t = u.time;
    // 0 before dawn → 1 high in the sky → 0 again at dusk.
    let e = 0.5 - 0.5 * cos(params().sun * TAU);

    let coral = palette.colors[1].rgb;
    let gold = palette.colors[2].rgb;
    let sunc = mix(coral, mix(gold, vec3<f32>(1.0, 0.95, 0.85), 0.5), smoothstep(0.1, 0.6, e));
    let sun_pos = vec2<f32>(-0.18 * u.aspect, HORIZON - 0.5 - 0.08 + e * 0.40);
    let sd = length(c - sun_pos);

    // Sky color at the horizon line (the sea reflects it too).
    let horizon_col = mix(coral * 0.06, mix(coral, gold, 0.6) * 0.45, smoothstep(0.0, 0.5, e)) + sunc * 0.05 * e;
    var col: vec3<f32>;

    if (p.y >= HORIZON) {
        // Sky: zenith from pre-dawn navy to morning blue; warm band at the
        // horizon; the sun and its glow.
        let zenith = mix(palette.colors[0].rgb * 0.025, vec3<f32>(0.22, 0.42, 0.85) * 0.11, smoothstep(0.15, 0.8, e));
        let band = exp(-(p.y - HORIZON) * mix(9.0, 4.0, e));
        col = mix(zenith, horizon_col, band);
        // Thin stratus, lit from below by the sun.
        let cl = smoothstep(0.55, 0.85, fbm(vec2<f32>(c.x * 1.1 + t * 0.006, p.y * 9.0)));
        col = col + cl * smoothstep(HORIZON + 0.05, HORIZON + 0.25, p.y) * (sunc * 0.12 * (0.2 + e) + palette.colors[0].rgb * 0.01);
        col = col + sunc * (fill(sd - 0.055) * (1.5 + 2.0 * e) + glow(sd - 0.055, 0.10) * 0.25 + glow(sd, 0.45) * 0.06) * smoothstep(0.0, 0.08, e + 0.04);

        // The headland, and the ship-shaped hotel on top of it.
        let hx = (p.x - 0.78) / 0.22;
        let cliff = HORIZON + 0.005 + 0.11 * smoothstep(0.0, 0.5, hx) + 0.015 * ridge(p.x * 9.0, 7.0, 0.8);
        let ship = sd_ship(c - vec2<f32>(0.42 * u.aspect, cliff - 0.5 - 0.004));
        if (hx > 0.0 && (p.y < cliff || ship < 0.0)) {
            col = palette.colors[0].rgb * 0.006 + horizon_col * 0.04;
            // Cabin lights, fading as the day comes.
            let win = vec2<f32>(c.x * 260.0, p.y * 200.0);
            let lit = step(0.55, rand2(floor(win))) * step(0.4, fract(win.x)) * step(0.4, fract(win.y));
            col = col + gold * lit * step(ship, -0.003) * step(cliff + 0.016, p.y) * (1.0 - e) * 0.25;
        }
    } else {
        // Sea: reflects the horizon far out, deepens to teal near the shore.
        let v = (HORIZON - p.y) / HORIZON;
        let deep = palette.colors[3].rgb * (0.012 + 0.05 * e) + horizon_col * 0.15;
        let sea = mix(horizon_col * 0.7, deep, smoothstep(0.0, 0.9, v));
        col = sea;

        // Crests, nearest first: the first whose (displaced) crest line is
        // above this pixel owns it. Crests keep their identity as they age,
        // so each carries one moment of the spectrum all the way in.
        let phase = fract(t / WAVE_SPACING);
        var crest_dist = 1e3;
        var crest_s = 0.0;
        var crest_v = 0.0;
        for (var k = WAVES - 1; k >= 0; k = k - 1) {
            let age = (f32(k) + phase) * WAVE_SPACING;
            let wv = wave_v(age);
            let base = HORIZON - wv * HORIZON;
            let amp = 0.16 * wv;
            if (base + amp < p.y) {
                continue; // even a full crest can't reach this pixel
            }
            let tx = clamp(0.5 + (p.x - 0.5) * (0.55 + 0.45 * wv), 0.0, 1.0);
            let s = spectrum_ago(tx, age);
            // A resting swell, so even quiet water rolls; the song lifts it.
            let swell = 0.22 + 0.12 * sin(p.x * (5.0 + 3.0 * rand2(vec2<f32>(f32(k), 1.0))) + f32(k) * 2.3);
            let crest = base + amp * (swell + 0.7 * s * s);
            if (crest >= p.y) {
                crest_dist = crest - p.y;
                crest_s = s;
                crest_v = wv;
                // Light through the lip, then the face falls into shadow
                // toward the trough in front of the next wave.
                let face = smoothstep(0.0, 0.05 * wv + 0.002, crest_dist);
                let fade = smoothstep(0.03, 0.2, wv);
                col = mix(sea, mix(sea * 1.35 + horizon_col * 0.08, sea * 0.6, face), fade);
                break;
            }
        }
        // Foam along the lip: loud moments break white; kicks flash the near ones.
        let lip = smoothstep(0.0025 * (0.4 + crest_v * 2.0), 0.0, crest_dist);
        let foam_col = mix(horizon_col * 2.0, vec3<f32>(0.9, 0.95, 1.0) * (0.05 + 0.25 * e), 0.5);
        col = col + foam_col * lip * (0.15 + crest_s * 1.5) * (1.0 + u.kick * 2.0 * crest_v) * smoothstep(0.0, 0.08, crest_v);

        // The sun's glitter path, widening toward the shore; hats glint.
        let path = exp(-pow((c.x - sun_pos.x) / (0.03 + v * 0.35), 2.0));
        let cell = floor(vec2<f32>(c.x * 220.0, p.y * 140.0 / max(v, 0.05)));
        let spark = step(0.93 - u.hat * 0.05, rand2(cell + floor(t * 12.0) * 3.7));
        col = col + sunc * spark * path * (0.08 + 0.25 * u.hat) * (0.15 + e);
    }

    // Decay is 0.5, so feedback doubles a steady image: halve to land on `col`.
    return vec4<f32>(col * 0.5, 1.0);
}
