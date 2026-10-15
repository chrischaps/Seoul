// Cheonggyecheon Lanterns — the stream through downtown Seoul during the
// lantern festival, seen from the bank. Lotus lanterns and blue-and-red
// silk lanterns drift downstream in four lanes; each lantern's inner glow
// is one spectrum bin, so the whole stream is a scattered EQ. More are set
// afloat as the song goes on. The current (`params().flow`) carries them and
// the custom warp (cheonggyecheon_lanterns_warp.wgsl) drags their light
// through the water at the same speed, like a long exposure. Kicks flare
// every lantern's flame; snares run a flash along the string lights on the
// upper walk, which twinkle with the hats. When the kicks stop (`groove`
// falls) the lanterns burn low and the current slackens until the drop.

const WATER_TOP: f32 = 0.70;
const WATER_BOT: f32 = 0.16;
const LANES: i32 = 4;
// Feedback keeps 0.85 a frame, so a steady image settles at 1 / 0.15 of the
// per-frame output: scale everything by this to land on the intended color.
const GAIN: f32 = 0.15;

fn lane_y(j: f32) -> f32 {
    return mix(0.25, 0.62, j / f32(LANES - 1));
}
// Shallow perspective: the far lanes are smaller and slower.
fn lane_scale(j: f32) -> f32 {
    return mix(1.0, 0.62, j / f32(LANES - 1));
}

// One lantern at `q` (centered coords relative to the lantern), size `s`.
// Returns (light rgb, coverage).
fn lantern(q: vec2<f32>, s: f32, seed: f32, level: f32) -> vec4<f32> {
    let hue = rand2(vec2<f32>(seed, 9.0));
    let glow_amt = 0.35 + 2.2 * pow(level, 1.5);
    if (rand2(vec2<f32>(seed, 10.0)) < 0.72) {
        // Lotus lantern: a scalloped bloom of petals around a bright heart.
        let silk = palette_ramp(hue * 0.999);
        let a = atan2(q.y, q.x);
        let r = length(q * vec2<f32>(1.0, 1.25)) / s;
        let edge = 1.0 + 0.16 * abs(sin(a * 4.0)) + 0.06 * abs(sin(a * 8.0 + 0.5));
        let d = (r - edge) * s;
        let cover = fill(d);
        // Two tiers of petals with dark seams between them, round a hot heart.
        let seam = smoothstep(0.0, 0.35, abs(sin(a * 4.0)));
        let inner_edge = 1.0 + 0.2 * abs(sin(a * 4.0 + PI * 0.25));
        let inner = fill((r / 0.6 - inner_edge) * s);
        let seam2 = smoothstep(0.0, 0.35, abs(sin(a * 4.0 + PI * 0.25)));
        let heart = exp(-r * r * 9.0);
        let light = (silk * cover * (0.12 + 0.45 * seam * smoothstep(0.2, 1.0, r))
            + silk * inner * (0.25 + 0.5 * seam2)
            + mix(silk, vec3<f32>(1.0, 0.95, 0.8), 0.6) * heart * 1.6) * glow_amt
            + silk * glow(d, s * 0.8) * 0.05 * glow_amt;
        return vec4<f32>(light, cover);
    }
    // Cheongsachorong: a blue-over-red silk lantern with a gold cap.
    let b = sd_box(q, vec2<f32>(s * 0.62, s * 0.9)) - s * 0.15;
    let cover = fill(b);
    let silk = select(vec3<f32>(0.95, 0.18, 0.15), vec3<f32>(0.15, 0.35, 1.0), q.y > s * 0.1);
    let cap = step(s * 0.85, abs(q.y)) * vec3<f32>(1.0, 0.75, 0.3);
    let heart = exp(-dot(q, q) / (s * s) * 2.0);
    let halo = vec3<f32>(0.6, 0.3, 0.6); // the two silks, blended in the air
    let light = (silk * (0.45 + heart) + cap * 0.8) * cover * glow_amt
        + halo * glow(b, s * 0.8) * 0.05 * glow_amt;
    return vec4<f32>(light, cover);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    let c = centered(p);
    let t = u.time;
    let flow = params().flow;
    let groove = params().groove;
    // Flame strength: low embers in a breakdown, flaring on each kick.
    let flame = (0.2 + 0.8 * groove) * (1.0 + u.kick * 2.0);
    // The stream fills up over the song.
    let density = 0.3 + 0.7 * min(u.beat_count / 96.0, 1.0);

    var col = vec3<f32>(0.0);

    if (p.y > WATER_TOP + 0.10) {
        // The walk above: night sky glow, a railing, and string lights
        // sagging between posts.
        col = palette.colors[0].rgb * 0.04 * (1.0 + (1.0 - p.y) * 2.0);
        let span = fract(p.x * 5.0);
        let sag = WATER_TOP + 0.17 - 0.035 * (1.0 - pow(2.0 * span - 1.0, 2.0));
        col = col + vec3<f32>(0.25, 0.2, 0.15) * smoothstep(0.0015, 0.0, abs(p.y - sag)) * 0.25;
        let bulb_x = (floor(p.x * 30.0) + 0.5) / 30.0;
        let bspan = fract(bulb_x * 5.0);
        let bulb = vec2<f32>((bulb_x - 0.5) * u.aspect, WATER_TOP + 0.17 - 0.035 * (1.0 - pow(2.0 * bspan - 1.0, 2.0)) - 0.5 - 0.008);
        let bd = length(c - bulb);
        let tw = 0.6 + 0.4 * sin(t * 3.0 + bulb_x * 77.0) * (0.3 + u.hat)
            + u.hat * step(0.75, rand2(vec2<f32>(bulb_x * 30.0, u.hat_count))) * 1.2;
        // A snare's flash runs along the walk from left to right.
        let run = exp(-pow((bulb_x - u.snare_age * 2.2) * 9.0, 2.0)) * exp(-u.snare_age * 1.5);
        col = col + palette.colors[2].rgb * (fill(bd - 0.0035) * 1.5 + glow(bd, 0.008) * 0.12) * tw
            + mix(palette.colors[2].rgb, vec3<f32>(1.0), 0.5) * (fill(bd - 0.0035) * 4.0 + glow(bd, 0.02) * 0.6) * run;
        let railing = step(WATER_TOP + 0.10, p.y) * step(p.y, WATER_TOP + 0.108);
        col = mix(col, vec3<f32>(0.03, 0.03, 0.04), railing);
    } else if (p.y > WATER_TOP || p.y < WATER_BOT) {
        // Granite embankments: offset courses of dressed blocks.
        let near = p.y < WATER_BOT;
        let scale = select(vec2<f32>(26.0, 70.0), vec2<f32>(16.0, 40.0), near);
        let row = floor(p.y * scale.y);
        let bq = vec2<f32>(p.x * scale.x + rand2(vec2<f32>(row, 3.0)) * 0.9, p.y * scale.y);
        let joint = step(0.06, fract(bq.x)) * step(0.1, fract(bq.y));
        let tone = 0.7 + 0.3 * rand2(floor(bq));
        // Lit warmly from the water.
        let lit = select(exp(-(p.y - WATER_TOP) * 30.0), exp(-(WATER_BOT - p.y) * 20.0), near);
        col = vec3<f32>(0.5, 0.48, 0.45) * tone * joint * (0.03 + 0.10 * lit * flame) * (0.6 + 0.4 * u.mid_att);
    } else {
        // Water: dark, with a slow glinting ripple.
        let rip = fbm(vec2<f32>(c.x * 6.0 - flow * 8.0, p.y * 40.0));
        col = palette.colors[0].rgb * 0.05 + vec3<f32>(0.08, 0.1, 0.16) * smoothstep(0.6, 0.9, rip) * (0.12 + u.hat * 0.5);

        // Stepping stones crossing the stream.
        let sx = 0.72 * u.aspect - 0.5 * u.aspect;
        for (var k = 0; k < 6; k = k + 1) {
            let sy = mix(WATER_BOT + 0.05, WATER_TOP - 0.04, f32(k) / 5.0) - 0.5;
            let sc = mix(1.0, 0.65, f32(k) / 5.0);
            let sd = sd_box(c - vec2<f32>(sx + (rand2(vec2<f32>(f32(k), 5.0)) - 0.5) * 0.03, sy), vec2<f32>(0.035, 0.016) * sc) - 0.008 * sc;
            col = mix(col, vec3<f32>(0.55, 0.52, 0.48) * 0.08, fill(sd));
        }

        // Lanterns, far lanes first so near ones float in front.
        for (var jj = LANES - 1; jj >= 0; jj = jj - 1) {
            let j = f32(jj);
            let sc = lane_scale(j);
            let size = 0.044 * sc;
            let gap = 0.19 * sc;
            let travel = flow * sc + rand2(vec2<f32>(j, 1.0));
            let fi = floor((travel - p.x) / gap);
            for (var n = -1; n <= 1; n = n + 1) {
                let i = fi + f32(n);
                let seed = i * 7.0 + j * 131.0;
                if (i < 0.0 || rand2(vec2<f32>(seed, 2.0)) > density) {
                    continue;
                }
                let lx = travel - i * gap + (rand2(vec2<f32>(seed, 3.0)) - 0.5) * gap * 0.5;
                let ly = lane_y(j) + (rand2(vec2<f32>(seed, 4.0)) - 0.5) * 0.05 * sc + 0.006 * sin(t * 0.9 + i);
                let center = vec2<f32>((lx - 0.5) * u.aspect, ly - 0.5);
                let level = spectrum_at(rand2(vec2<f32>(seed, 6.0)));
                // Its reflection: a long trembling streak below it.
                let rq = c - center + vec2<f32>(0.0, size * 1.6);
                let shimmer = 0.6 + 0.4 * sin(rq.y * 260.0 - t * 4.0);
                let refl = exp(-pow(rq.x / (size * 0.6), 2.0)) * exp(-max(-rq.y, 0.0) / (size * 2.5)) * step(rq.y, 0.0);
                let lc = lantern(c - center, size, seed, level);
                col = col + palette_ramp(rand2(vec2<f32>(seed, 9.0)) * 0.999) * refl * shimmer * 0.12 * (0.3 + level) * flame;
                let lit = lc.rgb * flame;
                col = mix(col, lit, lc.a) + lit * (1.0 - lc.a);
            }
        }
    }

    return vec4<f32>(col * GAIN, 1.0);
}
