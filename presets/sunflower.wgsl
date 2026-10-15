// Sunflower: log-spiral phyllotaxis. Seed n sits at gⁿ·e^(i·n·GA); the map
// z → g·e^(i·GA)·z sends every seed to the next, so the head is exactly
// self-similar and the dive never ends. A pixel only has to look at seeds
// whose radius is near its own: n ≈ ln r / ln g, give or take enough
// indices to reach the nearest neighbors around the circle (which sit a
// Fibonacci number of seeds apart).
//
// Seeds n, n+ARMS, n+2·ARMS… form one of the head's visible spirals, so
// lighting seed n by spectrum bin (n mod ARMS) gives each arm a note.

const GA: f32 = 2.39996323;   // golden angle
const G: f32 = 1.0065;        // radius ratio between consecutive seeds
const K: i32 = 34;            // neighbors checked on each side
const ARMS: f32 = 21.0;
// The dive moves FIB seeds per cycle. FIB golden angles is within 0.08 rad
// of a whole number of turns, so spreading that leftover over the cycle
// makes the seeds stream almost straight outward instead of spinning.
const FIB: f32 = 34.0;
const TURN: f32 = -0.0827380 / 34.0;

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p = rot2(q.spin) * centered(in.uv);
    let r = max(length(p), 1e-5);
    let lg = log(G);
    let base = FIB * floor(q.travel / FIB);
    let shift = q.travel - base;

    // Seed size relative to its radius: one seed per g-step fills the area.
    let size = sqrt(TAU * lg / PI) * (0.56 + q.hit * 0.14);

    var best = 1e9;
    var best_id = 0.0;
    var best_m = 0.0;
    let j0 = round(log(r) / lg + shift);
    for (var i = -K; i <= K; i = i + 1) {
        let j = j0 + f32(i);
        let m = j - shift;
        let rad = pow(G, m);
        let ang = j * GA - shift * TURN;
        let c = rad * vec2<f32>(cos(ang), sin(ang));
        let d = length(p - c) / (rad * size);
        if (d < best) {
            best = d;
            best_id = j + base;
            best_m = m;
        }
    }

    let rad = pow(G, best_m);
    let px = 1.0 / (u.resolution.y * rad * size);
    let disk = smoothstep(1.0 + px, 1.0 - px, best);
    let dome = sqrt(max(1.0 - best * best, 0.0));

    // Inner florets are dark and green-brown, the outer ones gold.
    let ring = smoothstep(0.02, 0.45, rad);
    let arm = (best_id - ARMS * floor(best_id / ARMS)) / ARMS;
    let note = spectrum_at(arm);
    let hue = mix(palette.colors[0].rgb, mix(palette.colors[2].rgb, palette.colors[1].rgb, ring), 0.35 + 0.65 * ring);

    // Kicks ripple outward through the head.
    let ripple_r = 0.02 * exp(u.kick_age * 5.0);
    let ripple = exp(-pow((log(rad) - log(ripple_r)) * 2.5, 2.0)) * exp(-u.kick_age * 1.5);

    let energy = 0.004 + q.groove * 0.016 + note * (0.015 + q.groove * 0.045) + ripple * 0.08 + q.hit * 0.012;
    var col = hue * disk * dome * energy;
    // Snares: a burst of pollen, new seeds each hit. Hats: a few twinkle.
    let burst = step(0.80, rand2(vec2<f32>(best_id, u.snare_count))) * q.clap;
    let twinkle = step(0.95, rand2(vec2<f32>(best_id, u.hat_count + 0.5))) * q.tick;
    let glint = burst * 0.12 + twinkle * 0.06 + step(0.93, rand2(vec2<f32>(best_id, 3.0))) * q.pollen * 0.03;
    col = col + palette.colors[3].rgb * disk * pow(dome, 4.0) * glint;

    // Petals ring the head, fixed in place while the seeds stream through:
    // two rows, the back row offset by half a petal and dimmer.
    let pr = length(p);
    let pa = atan2(p.y, p.x) / TAU;
    var petals = 0.0;
    for (var row = 0; row < 2; row = row + 1) {
        let fr = f32(row);
        let count = 21.0;
        let t = pa * count + fr * 0.5;
        let pf = (fract(t) - 0.5) * 2.0;
        let seed = rand2(vec2<f32>(floor(t), fr));
        let len = (0.19 + seed * 0.06) * (1.0 - fr * 0.12) * (1.0 + q.hit * 0.10 + q.groove * 0.04);
        let x = (pr - 0.43) / len;
        let width = sqrt(max(x, 0.0)) * max(1.0 - x, 0.0) * 1.5;
        let d = abs(pf) - width * 0.8;
        let shade = (0.6 + 0.4 * (1.0 - abs(pf))) * (1.0 - fr * 0.45) * (0.6 + 0.4 * x);
        petals = max(petals, smoothstep(0.08, -0.08, d) * step(0.0, x) * step(x, 1.0) * shade);
    }
    let head = 1.0 - smoothstep(0.425, 0.44, pr);
    col = col * head + mix(palette.colors[2].rgb, palette.colors[1].rgb, 0.7) * petals * (1.0 - head) * (0.008 + q.groove * 0.02 + q.hit * 0.012);
    return vec4<f32>(col, 1.0);
}
