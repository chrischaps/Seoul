// Window Within: the Droste effect. The picture between the window's outer
// edge (half-size H) and its opening (H/S) is drawn once; the opening
// holds the same picture scaled by 1/S, forever. In log space that's a
// pattern periodic in ln S along the real axis and 2π along the imaginary.
// Escher's map w → α·w with α = 1 + ln S/(2πi) makes one turn around the
// center descend one level, turning the nested frames into a spiral.
// Sliding along the real axis by `dive`·ln S falls one level per unit;
// the picture at dive = 1 is the picture at 0, so the loop is exact.

const S: f32 = 4.0;
const H: f32 = 0.46;          // window half-size
const FRAME: f32 = 0.05;      // wooden frame width

// Ttisal (띠살), the classic hanok door lattice: slender vertical bars
// close together, crossed by horizontal bars gathered in bands of three
// near the top, middle and bottom.
fn lattice_dist(z: vec2<f32>) -> f32 {
    let inner = H - FRAME;
    let gv = inner * 2.0 / 16.0;
    let vx = abs(fract((z.x + inner) / gv + 0.5) - 0.5) * gv;
    var hy = 1e9;
    for (var b = -1; b <= 1; b = b + 1) {
        let c = f32(b) * inner * 0.7;
        for (var k = -1; k <= 1; k = k + 1) {
            hy = min(hy, abs(z.y - c - f32(k) * gv * 0.55));
        }
    }
    return min(vx, hy);
}

// A plum branch's shadow falling across the paper from the moon outside.
fn branch_dist(z: vec2<f32>) -> vec2<f32> {
    let a = vec2<f32>(-0.48, -0.30);
    let b = vec2<f32>(-0.18, -0.12);
    let c = vec2<f32>(0.05, 0.02);
    let d = vec2<f32>(0.22, 0.20);
    let e = vec2<f32>(-0.05, 0.22);
    let f = vec2<f32>(0.10, -0.20);
    var wood = sd_segment(z, a, b) - 0.012;
    wood = min(wood, sd_segment(z, b, c) - 0.008);
    wood = min(wood, sd_segment(z, c, d) - 0.005);
    wood = min(wood, sd_segment(z, c, e) - 0.004);
    wood = min(wood, sd_segment(z, b, f) - 0.005);
    var bloom = 1e9;
    bloom = min(bloom, sd_circle(z - d, 0.022));
    bloom = min(bloom, sd_circle(z - e, 0.018));
    bloom = min(bloom, sd_circle(z - f, 0.02));
    bloom = min(bloom, sd_circle(z - mix(c, d, 0.5) - vec2<f32>(0.02, -0.02), 0.016));
    bloom = min(bloom, sd_circle(z - mix(b, c, 0.4) + vec2<f32>(0.0, 0.03), 0.017));
    return vec2<f32>(wood, bloom);
}

// The window at `z` (one level), with `px` the size of a pixel there.
fn window(z: vec2<f32>, px: f32) -> vec3<f32> {
    let q = params();
    let outer = sd_box(z, vec2<f32>(H));
    let aa = px * 1.2;

    // Paper inside the frame, lit from behind by a lamp off to one side.
    let paper = smoothstep(aa, -aa, outer + FRAME);
    let lamp_at = vec2<f32>(0.12, -0.06);
    let lamp = q.lamp * (0.3 + 0.9 * exp(-dot(z - lamp_at, z - lamp_at) * 7.0));
    let hanji = 0.82 + 0.18 * vnoise(z * 70.0) + 0.06 * vnoise(z * 300.0);

    let ld = lattice_dist(z);
    let bar = 0.0032;
    let lattice = smoothstep(bar + aa, bar - aa, ld);
    let shimmer = 1.0 + (q.shim * 0.5 + q.tick * 2.0) * (0.5 + 0.5 * sin(z.x * 40.0 + z.y * 55.0 + drum(HAT).count * 2.3));

    // The branch shadow is soft (the moon is a disk, not a point).
    let bd = branch_dist(z);
    let shadow = 1.0 - 0.55 * smoothstep(0.008, -0.004, min(bd.x, bd.y));

    // Lamplight while the music drives; cool moonlight in a breakdown.
    var light = mix(palette.colors[1].rgb, vec3<f32>(1.0, 0.62, 0.30), q.warm * 0.4);
    light = mix(palette.colors[3].rgb * vec3<f32>(0.55, 0.65, 1.0), light, 0.35 + 0.65 * q.groove);
    var col = light * paper * (1.0 - lattice) * hanji * lamp * shadow * 0.022;
    // Snares light the plum blossoms in the shadow.
    let blossom = smoothstep(0.004, -0.004, bd.y);
    col = col + vec3<f32>(1.0, 0.45, 0.6) * paper * blossom * q.clap * 0.05;
    // The bars catch a little light at their edges.
    col = col + palette.colors[2].rgb * paper * lattice * 0.006 * shimmer * q.lamp;

    // The wooden frame.
    let frame = smoothstep(aa, -aa, max(outer, -(outer + FRAME)));
    let grain = 0.6 + 0.4 * vnoise(vec2<f32>(dot(z, vec2<f32>(1.0)) * 6.0, max(abs(z.x), abs(z.y)) * 260.0));
    col = col + palette.colors[2].rgb * frame * grain * 0.012;
    // Light runs around the frame's edges, faster on beats.
    let ang = atan2(z.y, z.x);
    let run = pow(0.5 + 0.5 * cos(ang * 2.0 - drum(SNARE).count * 1.7 - (1.0 - q.clap) * 6.0), 10.0);
    let edge = exp(-abs(outer + FRAME) / (px * 2.5)) + exp(-abs(outer) / (px * 2.5));
    col = col + palette.colors[3].rgb * edge * (run * (0.002 + q.clap * 0.04) + q.hit * 0.004);
    return col;
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p = rot2(q.spin) * centered(in.uv);
    let ls = log(S);
    let alpha = vec2<f32>(1.0, -ls / TAU);

    var w = cmul(clog(p), alpha);
    w.x = w.x - q.dive * ls;
    // Wrap into one level's band of radii, then nudge across the square
    // edges so z lands between the opening and the outer edge.
    let lo = log(H / S);
    w.x = lo + (w.x - lo) - floor((w.x - lo) / ls) * ls;
    var z = cexp(w);
    var px = length(alpha) * length(z) / max(length(p), 1e-6) / u.resolution.y;
    for (var i = 0; i < 2; i = i + 1) {
        if (sd_box(z, vec2<f32>(H)) > 0.0) {
            z = z / S;
            px = px / S;
        }
        if (sd_box(z, vec2<f32>(H / S)) < 0.0) {
            z = z * S;
            px = px * S;
        }
    }
    return vec4<f32>(window(z, px), 1.0);
}
