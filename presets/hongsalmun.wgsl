// Hongsalmun: walking through an endless row of red arrow gates. Gate k
// stands at depth k·D; the eye is at walk·D, so gate k is (k − walk)·D
// ahead and appears at 1/depth scale (a perspective divide). Gates are
// drawn far to near, each replacing what's behind it, and fade in out of
// the haze so the far end never pops. The gate index seeds each gate's
// small differences, so the row stays the same as you walk it.

const D: f32 = 1.9;           // spacing between gates
const GATES: i32 = 12;
const HORIZON: f32 = 0.03;
const EYE: f32 = 0.42;        // eye height above the path
const W: f32 = 0.62;          // half-width between the pillars
const BEAM: f32 = 1.25;       // height of the main crossbeam
const PULSE: f32 = 9.0;       // how fast a kick's light runs ahead
const MOON: vec3<f32> = vec3<f32>(0.80, 0.86, 1.0);

// One gate in its own world units (x across, y up from the path).
// Returns (lacquer, arrow tips, taegeuk) coverage, each 0..1.
fn gate(p: vec2<f32>, px: f32, seed: f32) -> vec3<f32> {
    let aa = px * 1.5;
    let x = abs(p.x);
    // Pillars, a little thinner toward the top, on stone bases.
    var d = sd_box(vec2<f32>(x - W, p.y - BEAM * 0.5), vec2<f32>(0.035 - p.y * 0.006, BEAM * 0.5 + 0.08));
    d = min(d, sd_box(vec2<f32>(x - W, p.y - 0.04), vec2<f32>(0.06, 0.04)));
    // Two crossbeams.
    d = min(d, sd_box(vec2<f32>(p.x, p.y - BEAM), vec2<f32>(W + 0.07, 0.022)));
    d = min(d, sd_box(vec2<f32>(p.x, p.y - BEAM + 0.16), vec2<f32>(W, 0.014)));
    // The red arrows: slim vertical shafts between and above the beams.
    let n = 15.0;
    let sp = W * 2.0 / n;
    let ax = (fract(p.x / sp + 0.5) - 0.5) * sp;
    let top = BEAM + 0.30 + 0.02 * sin(floor(p.x / sp + 0.5) * 2.1 + seed * 6.0);
    let shaft = sd_box(vec2<f32>(ax, p.y - (BEAM + top - 0.18) * 0.5), vec2<f32>(0.006, (top - BEAM + 0.18) * 0.5));
    let inside = step(abs(p.x), W - sp * 0.4);
    d = min(d, select(1e9, shaft, inside > 0.5));
    let lacquer = smoothstep(aa, -aa, d);

    // Arrowheads at the shafts' tips.
    let tip_p = vec2<f32>(ax, p.y - top);
    let tip_d = max(abs(tip_p.x) * 2.2 + tip_p.y - 0.05, -tip_p.y);
    let tips = smoothstep(aa, -aa, tip_d) * inside;

    // The taegeuk above the center, with a trident through it.
    let tc = vec2<f32>(p.x, p.y - (BEAM + 0.30));
    let disk = sd_circle(tc, 0.075);
    let taegeuk = smoothstep(aa, -aa, disk);
    return vec3<f32>(lacquer, tips, taegeuk);
}

// Red above, blue below, the two halves curling into each other.
fn taegeuk_color(p: vec2<f32>) -> vec3<f32> {
    let r = 0.075;
    let upper = sd_circle(p - vec2<f32>(-r * 0.5, 0.0), r * 0.5);
    let lower = sd_circle(p - vec2<f32>(r * 0.5, 0.0), r * 0.5);
    var red = p.y > 0.0;
    if (upper < 0.0) { red = true; }
    if (lower < 0.0) { red = false; }
    return select(palette.colors[2].rgb, palette.colors[1].rgb, red);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    var s = centered(in.uv);
    s.x = s.x + sin(u.time * 0.4) * 0.04 * q.sway;
    let px = 1.0 / u.resolution.y;

    // Sky: a faint glow at the horizon, and stars.
    var col = palette.colors[0].rgb * 0.004 * exp(-max(s.y - HORIZON, 0.0) * 4.0);
    let star = stars(s + vec2<f32>(q.walk * 0.002, 0.0), 60.0, 0.12);
    let twinkle = 0.5 + 1.5 * q.tick * step(0.5, rand2(floor((s + vec2<f32>(q.walk * 0.002, 0.0)) * 60.0) + drum(HAT).count));
    col = col + palette.colors[3].rgb * star * 0.008 * twinkle * step(HORIZON, s.y);

    // A full moon low over the end of the path; the gates pass in front of it.
    let mp = s - vec2<f32>(0.0, HORIZON + 0.13);
    let moon = smoothstep(0.062, 0.058, length(mp)) * (0.85 + 0.15 * vnoise(mp * 60.0));
    let halo = exp(-max(length(mp) - 0.06, 0.0) * 9.0);
    col = col + MOON * (moon * (0.024 + q.hit * 0.012) + halo * (0.003 + q.glow * 0.003 + q.hit * 0.008));

    // The stone path: tiles in perspective, sliding toward you, with the
    // moon's reflection running down its middle.
    if (s.y < HORIZON) {
        let depth = EYE / (HORIZON - s.y);
        let world = vec2<f32>(s.x * depth, depth + q.walk * D);
        let path = smoothstep(W * 0.95, W * 0.75, abs(world.x));
        let row = floor(world.y * 1.4);
        let tile = fract(world * vec2<f32>(1.8, 1.4) + vec2<f32>(0.5 * row, 0.0));
        let edge = min(min(tile.x, 1.0 - tile.x), min(tile.y, 1.0 - tile.y));
        let seam = smoothstep(0.0, 0.05, edge);
        let stone = 0.7 + 0.3 * rand2(floor(world * vec2<f32>(1.8, 1.4) + vec2<f32>(0.5 * row, 0.0)));
        let fog = exp(-depth * 0.15);
        let sheen = exp(-abs(s.x) * 14.0 / (1.0 + depth * 0.2)) * (0.3 + 0.7 * fog);
        col = MOON * path * seam * stone * (fog * 0.0035 + sheen * 0.004) + palette.colors[0].rgb * 0.003 * fog;
    }

    let base = floor(q.walk);
    for (var i = 0; i < GATES; i = i + 1) {
        let k = base + f32(GATES - i);
        let z = (k - q.walk) * D;
        if (z < 0.15) {
            continue;
        }
        let local = vec2<f32>(s.x * z, (s.y - HORIZON) * z + EYE);
        let g = gate(local, px * z, rand2(vec2<f32>(k, 4.0)));
        let cover = max(max(g.x, g.y), g.z);
        if (cover <= 0.0) {
            continue;
        }

        let fog = exp(-z * 0.12);
        let fade = smoothstep(f32(GATES) * D, f32(GATES - 2) * D, z) * smoothstep(0.3, 1.1, z);
        // A kick's light reaches this gate when it has run z ahead.
        let pulse = exp(-pow((z - q.kick_t * PULSE) / 1.2, 2.0)) * exp(-q.kick_t * 0.8);
        let lit = 0.004 + q.glow * 0.010 + pulse * 0.09;

        var gc = palette.colors[1].rgb * g.x * lit * (0.7 + 0.3 * rand2(vec2<f32>(k, 9.0)));
        gc = gc + palette.colors[3].rgb * g.y * (0.006 + q.groove * 0.004 + q.clap * 0.10 + pulse * 0.04);
        let tc = vec2<f32>(local.x, local.y - (BEAM + 0.30));
        gc = mix(gc, taegeuk_color(tc) * (lit * 1.4 + 0.004 + q.clap * 0.05), g.z);
        let haze = palette.colors[0].rgb * 0.004;
        gc = mix(haze, gc, fog);
        col = mix(col, gc, cover * fade);
    }

    return vec4<f32>(col, 1.0);
}
