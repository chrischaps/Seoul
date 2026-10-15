// Hutchinson warp: an iterated function system run by the feedback loop.
// Each map f(x) = v + s·R(twist)·(x − v) shrinks the plane toward a vertex
// v; this pixel shows the brightest of last frame's pixels that the maps
// carry here. Each map also tints a little, so the attractor ends up colored
// by its "address": which corner each sub-triangle descends from.
//
// The dive: the whole system is seen through Z = f_k^(−dive), a fractional
// power of the map for corner k. The attractor A satisfies A ⊆ f_k⁻¹(A), so
// as `dive` runs 0 → 1 the view falls one level into corner k and lands on
// the same picture it started from; then `dive` wraps (and the next corner
// is chosen) with no seam. An endless fall into the fractal.

const RADIUS: f32 = 0.43;

fn tint(i: i32) -> vec3<f32> {
    let c = palette.colors[i].rgb;
    return mix(vec3<f32>(1.0), c / max(max(c.r, c.g), max(c.b, 1e-3)), 0.14);
}

fn vertex(turn: f32, i: i32) -> vec2<f32> {
    let a = turn + f32(i) * TAU / 3.0 + PI * 0.5;
    return RADIUS * vec2<f32>(cos(a), sin(a));
}

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let back = rot2(-q.twist);

    // Z maps attractor space to the screen: about corner v0, scale by
    // s^(−dive) and turn by −twist·dive.
    let v0 = vertex(q.turn, i32(q.corner));
    let k = pow(q.scale, -q.dive);
    let zr = rot2(-q.twist * q.dive);
    let zr_inv = rot2(q.twist * q.dive);
    let x = v0 + zr_inv * (centered(in.uv) - v0) / k;

    var best = vec3<f32>(0.0);
    for (var i = 0; i < 3; i = i + 1) {
        let v = vertex(q.turn, i);
        // Inverse map: where in last frame this pixel's light came from.
        let y = v + back * (x - v) / q.scale;
        best = max(best, sample_prev(uncentered(v0 + zr * (y - v0) * k)) * tint(i + 1));
    }

    // The center map spins its copy, which turns the triangle into a spiral.
    let yc = rot2(-q.psi) * x / q.cscale;
    best = max(best, sample_prev(uncentered(v0 + zr * (yc - v0) * k)) * q.cweight);

    return finish(in.uv, best);
}
