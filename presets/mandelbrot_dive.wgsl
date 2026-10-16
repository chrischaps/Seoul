// Misiurewicz: a dive into the Mandelbrot set at c₀ ≈ −0.1011 + 0.9563i.
// c₀'s orbit is 0 → c₀ → z₂ → −β → β → β → …, where β is a repelling fixed
// point with multiplier λ = 2β (|λ| ≈ 1.33, turning 120° per step). Near c₀
// the set looks the same at scale ε and ε/λ, with each escape count one
// higher. LEVEL = λ³ (×2.34, almost no turn) is one cycle of `dive`, so
// count − 3·dive is continuous across the wrap.
//
// Deep zooms outrun f32, so the orbit is computed as a perturbation δ
// around c₀'s own orbit, which is exact and known in closed form. When
// z = Z + δ dips below |δ|, the orbit is rebased onto the start of the
// reference (Zhuoran's trick) so precision never drains away.

const C0: vec2<f32> = vec2<f32>(-0.10109636384562216, 0.9562865108091415);
const BETA: vec2<f32> = vec2<f32>(-0.3275861786993566, 0.5777564532989494);
// log(λ³), principal branch: a near-pure zoom of ×2.344 per level.
const LOG_LEVEL: vec2<f32> = vec2<f32>(0.8517678028847221, -0.023399272963458614);
const SHIFT: f32 = 3.0;       // escape-count increase per level
const BASE: f32 = 0.004;      // view height at dive = 0
const MAX_ITER: i32 = 220;
const BAIL: f32 = 1024.0;

fn reference(n: i32) -> vec2<f32> {
    switch n {
        case 0: { return vec2<f32>(0.0); }
        case 1: { return C0; }
        case 2: { return vec2<f32>(-1.005359779818338, 0.7629323327342986); }
        case 3: { return -BETA; }
        default: { return BETA; }
    }
}

// Returns (smooth count, distance estimate, escaped, closest approach to β).
fn misiurewicz(dc: vec2<f32>) -> vec4<f32> {
    var d = vec2<f32>(0.0);       // δ
    var dz = vec2<f32>(0.0);      // dz/dc
    var n = 0;
    var near = 1e9;
    for (var i = 0; i < MAX_ITER; i = i + 1) {
        let zr = reference(n);
        let z = zr + d;
        dz = 2.0 * cmul(z, dz) + vec2<f32>(1.0, 0.0);
        d = cmul(2.0 * zr + d, d) + dc;
        n = n + 1;
        let zn = reference(n) + d;
        let r2 = dot(zn, zn);
        near = min(near, length(zn - BETA));
        if (r2 > BAIL * BAIL) {
            let r = sqrt(r2);
            let smooth_n = f32(i) + 1.0 - log2(log(r) / log(BAIL));
            let de = r * log(r) / max(length(dz), 1e-30);
            return vec4<f32>(smooth_n, de, 1.0, near);
        }
        if (r2 < dot(d, d)) {
            d = zn;
            n = 0;
        }
    }
    return vec4<f32>(f32(MAX_ITER), 0.0, 0.0, near);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p = rot2(q.spin) * centered(in.uv);
    let scale = cexp(-q.dive * LOG_LEVEL) * BASE;
    let dc = cmul(p, scale);
    let px = length(scale) / u.resolution.y;

    let m = misiurewicz(dc);
    var col = palette.colors[0].rgb * 0.002;
    if (m.z > 0.5) {
        let n = m.x - SHIFT * q.dive;
        // Filaments: distance to the set in pixels.
        let de_px = m.y / px;
        let wire = exp(-de_px * 0.6);
        let halo = exp(-de_px * 0.04);
        let hue = cosine_palette(n * 0.045 + q.hue + u.cv[1] * 0.5, 0.2);
        // Breakdowns cool the glow toward the palette's blue.
        let tint = mix(palette.colors[1].rgb, hue, 0.4 + 0.6 * q.groove);
        col = col + tint * halo * (0.007 + 0.009 * q.groove + q.hit * 0.008 + max(u.cv[0], 0.0) * 0.008);
        // Hats: sparks travel along the filaments, each hit a new pattern.
        let spark = step(0.86, vnoise(vec2<f32>(n * 3.0 - u.time * 6.0, drum(HAT).count * 1.7))) * q.tick;
        let flare = 0.006 + q.groove * 0.004 + q.shine * 0.02 + max(u.cv[0], 0.0) * 0.012 + q.hit * 0.05 + spark * 0.06;
        col = col + mix(palette.colors[3].rgb, hue, 0.55) * wire * flare;
    }
    return vec4<f32>(col, 1.0);
}
