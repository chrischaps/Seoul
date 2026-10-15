// Fractal helpers, shared by composite and warp shaders (after common.wgsl).
// Complex numbers are vec2<f32> (x = real, y = imaginary).

// ---- Complex arithmetic -----------------------------------------------------

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn cdiv(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x + a.y * b.y, a.y * b.x - a.x * b.y) / max(dot(b, b), 1e-20);
}

fn csq(z: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y);
}

fn conj(z: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(z.x, -z.y);
}

fn cabs2(z: vec2<f32>) -> f32 {
    return dot(z, z);
}

fn cexp(z: vec2<f32>) -> vec2<f32> {
    return exp(z.x) * vec2<f32>(cos(z.y), sin(z.y));
}

fn clog(z: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(0.5 * log(max(dot(z, z), 1e-30)), atan2(z.y, z.x));
}

// Principal square root.
fn csqrt(z: vec2<f32>) -> vec2<f32> {
    let r = length(z);
    let s = select(1.0, -1.0, z.y < 0.0);
    return vec2<f32>(sqrt(max(0.5 * (r + z.x), 0.0)), s * sqrt(max(0.5 * (r - z.x), 0.0)));
}

// z^n for real n (principal branch).
fn cpow(z: vec2<f32>, n: f32) -> vec2<f32> {
    let r = pow(max(length(z), 1e-15), n);
    let a = atan2(z.y, z.x) * n;
    return r * vec2<f32>(cos(a), sin(a));
}

// Möbius automorphism of the unit disk that carries 0 to `a` (|a| < 1).
// Moving `a` glides the view through the hyperbolic plane.
fn disk_move(z: vec2<f32>, a: vec2<f32>) -> vec2<f32> {
    return cdiv(z + a, vec2<f32>(1.0, 0.0) + cmul(conj(a), z));
}

// ---- Geometry ---------------------------------------------------------------

// Counter-clockwise rotation by `a` radians.
fn rot2(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

// Reflect `p` to the positive side of the line through the origin with
// unit normal `n` — the basic move of fold-built fractals (Koch, trees).
fn fold_plane(p: vec2<f32>, n: vec2<f32>) -> vec2<f32> {
    return p - 2.0 * min(0.0, dot(p, n)) * n;
}

// Invert `p` in the circle (center, r).
fn circle_invert(p: vec2<f32>, center: vec2<f32>, r: f32) -> vec2<f32> {
    let d = p - center;
    return center + d * (r * r / max(dot(d, d), 1e-20));
}

// Distance from `p` to the segment a→b.
fn segment_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-20), 0.0, 1.0);
    return length(pa - ba * h);
}

// ---- Escape-time ------------------------------------------------------------

// Iterate z ← z² + c from `z0` (Julia: z0 = pixel, c fixed; Mandelbrot:
// z0 = 0, c = pixel). Returns
//   x: smooth iteration count (continuous, no bands); `max_iter` if bounded
//   y: orbit trap, the orbit's closest approach to the circle |z| = trap_r
//   z: 1 if the orbit escaped, else 0
//   w: the orbit's closest approach to 0
fn julia(z0: vec2<f32>, c: vec2<f32>, max_iter: i32, trap_r: f32) -> vec4<f32> {
    var z = z0;
    var trap = 1e9;
    var near = 1e9;
    for (var i = 0; i < max_iter; i = i + 1) {
        z = csq(z) + c;
        let m2 = dot(z, z);
        let m = sqrt(m2);
        trap = min(trap, abs(m - trap_r));
        near = min(near, m);
        if (m2 > 256.0) {
            // Continuous escape count: subtract how far past the bailout we landed.
            let n = f32(i) + 1.0 - log2(log2(m2) * 0.5);
            return vec4<f32>(n, trap, 1.0, near);
        }
    }
    return vec4<f32>(f32(max_iter), trap, 0.0, near);
}

// The more repelling fixed point of z² + c (z = (1 ± √(1 − 4c)) / 2). It
// always lies on the Julia set, which makes it a good place to zoom toward;
// its multiplier is 2z.
fn julia_beta(c: vec2<f32>) -> vec2<f32> {
    let s = csqrt(vec2<f32>(1.0, 0.0) - 4.0 * c);
    let za = (vec2<f32>(1.0, 0.0) + s) * 0.5;
    let zb = (vec2<f32>(1.0, 0.0) - s) * 0.5;
    return select(zb, za, dot(za, za) >= dot(zb, zb));
}

// Kali set ("Star Nest" in 2D): p ← |p| / |p|² − c. Returns
//   x: accumulated change in |p| — filaments and nebula density
//   y: the orbit's closest approach to the origin (a star-like trap)
fn kali(p_in: vec2<f32>, c: vec2<f32>, iters: i32) -> vec2<f32> {
    var p = p_in;
    var acc = 0.0;
    var prev = 0.0;
    var near = 1e9;
    for (var i = 0; i < iters; i = i + 1) {
        p = abs(p) / max(dot(p, p), 1e-6) - c;
        let l = length(p);
        acc = acc + abs(l - prev);
        prev = l;
        near = min(near, l);
    }
    return vec2<f32>(acc, near);
}

// ---- Color ------------------------------------------------------------------

// Cycle through the preset palette smoothly and seamlessly: `t` is any real
// number (1 = one full cycle out and back), `phase` offsets it.
fn cosine_palette(t: f32, phase: f32) -> vec3<f32> {
    return palette_ramp(0.5 - 0.5 * cos(TAU * (t + phase)));
}
