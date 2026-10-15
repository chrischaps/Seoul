// Sonamu: walking through a moonlit pine forest. Each pine is a fold-built
// tree: one segment is the trunk; each level moves to the tip, bends with
// the wind, mirrors (x → |x|, making two branches), turns outward and
// shrinks, so one distance test draws every branch. The upper levels are
// needles: soft jade clouds, the way pines are painted.
//
// The walk: TREES pines stand at staggered depths along the path. Each
// comes out of the haze near the horizon, grows in perspective as you
// approach, and passes off to one side; then it's replanted far ahead with
// a new position and shape. The moon is infinitely far, so it never moves:
// that parallax is what reads as depth.

const LEVELS: i32 = 9;
const SEG: f32 = 0.16;
const RATIO: f32 = 0.74;
const TREES: i32 = 4;
const HORIZON: f32 = -0.08;
const EYE: f32 = 0.42;     // eye height above the ground
const FAR: f32 = 5.0;
const NEAR: f32 = 0.6;

// Distances to one tree's wood and needles, in the tree's own units.
fn pine(p_in: vec2<f32>, spread: f32, bend: f32, crook_sign: f32) -> vec2<f32> {
    var p = p_in;
    var s = 1.0;
    var wood = 1e9;
    var leaf = 1e9;
    for (var i = 0; i < LEVELS; i = i + 1) {
        let fi = f32(i);
        // A tall trunk, then shorter limbs.
        let seg = select(SEG, SEG * 1.5, i == 0);
        let d = (segment_dist(p, vec2<f32>(0.0), vec2<f32>(0.0, seg)) - 0.026) * s;
        if (i >= 6) {
            leaf = min(leaf, d);
        } else {
            wood = min(wood, d);
        }
        p.y = p.y - seg;
        // The trunk zigzags (red pines grow crooked); the wind bends the
        // higher levels more.
        let crook = select(0.0, 0.16 * crook_sign * (1.0 - 2.0 * f32(i % 2)), i < 3);
        p = rot2(crook + bend * (1.0 + fi * 0.35)) * p;
        p.x = abs(p.x);
        // Upper limbs turn less, so the crown flattens instead of curling over.
        p = rot2(spread * (1.0 - fi * 0.06)) * p;
        p = p / RATIO;
        s = s * RATIO;
    }
    return vec2<f32>(wood, leaf);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p0 = centered(in.uv);
    let px = 1.0 / u.resolution.y;

    var col = vec3<f32>(0.0);
    var cover = 0.0;
    for (var i = 0; i < TREES; i = i + 1) {
        let t = q.walk + f32(i) / f32(TREES);
        let phase = fract(t);
        let id = floor(t) * f32(TREES) + f32(i);
        let h1 = rand2(vec2<f32>(id, 3.0));
        let h2 = rand2(vec2<f32>(id, 11.0));

        // Depth falls from FAR to NEAR; perspective divides by it.
        let z = mix(FAR, NEAR, phase);
        let side = select(-1.0, 1.0, h1 > 0.5);
        let x_world = side * (0.35 + h2 * 0.9);
        let base = vec2<f32>(x_world / z, HORIZON - EYE / z);
        let local = (p0 - base) * z;

        let d = pine(local, q.spread * (0.85 + h2 * 0.3), q.bend, side) / z;

        // Fade in out of the haze, and out just before passing.
        let haze = smoothstep(0.0, 0.25, phase) * (1.0 - smoothstep(0.9, 1.0, phase));
        let near = 1.0 / (1.0 + z * 0.35);

        let wood_mask = smoothstep(px * 1.5, -px * 1.5, d.x);
        let tuft = d.y + (fbm(local * 45.0) - 0.5) * 0.016 / z;
        let leaf_mask = smoothstep(0.012 / z, 0.0, tuft);

        let w = haze * near;
        let glitter = step(0.985, rand2(floor(local * 160.0) + vec2<f32>(u.hat_count, id))) * u.hat;
        col = col + palette.colors[1].rgb * wood_mask * (0.012 + q.glow * 0.012) * w;
        col = col + palette.colors[2].rgb * leaf_mask * (0.006 + q.glow * 0.018 + q.gust * 0.09 + glitter * 0.4) * w;
        cover = max(cover, max(wood_mask, leaf_mask) * haze);
    }

    // Moon, hidden where a tree stands in front of it.
    let mp = p0 - vec2<f32>(0.26 * u.aspect, 0.22);
    let moon = smoothstep(0.125, 0.118, length(mp));
    let halo = exp(-max(length(mp) - 0.12, 0.0) * 14.0);
    // In breakdowns clouds drift across the moon; the drop clears them.
    // Kicks make the moonlight breathe; a snare sends a ring of light out.
    let cloud = smoothstep(0.35, 0.75, fbm(mp * vec2<f32>(4.0, 9.0) + vec2<f32>(u.time * 0.05, 0.0)));
    let veil = 1.0 - (1.0 - q.groove) * (0.35 + 0.6 * cloud);
    let ring = exp(-pow((length(mp) - 0.13 - u.snare_age * 0.6) / 0.012, 2.0)) * exp(-u.snare_age * 3.0);
    col = col + palette.colors[3].rgb * ((moon * 0.010 * (1.0 + q.gust * 0.5) + halo * (0.002 + q.gust * 0.016)) * veil + ring * 0.01) * (1.0 - cover);

    // A faint moonlit path toward the horizon.
    let ground = smoothstep(HORIZON, HORIZON - 0.5, p0.y);
    let path = exp(-abs(p0.x) * 6.0 / max(HORIZON - p0.y, 0.02)) * ground;
    col = col + palette.colors[3].rgb * path * 0.0015;

    return vec4<f32>(col, 1.0);
}
