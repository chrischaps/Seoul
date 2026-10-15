// Julia Bloom: escape-time Julia set with continuous coloring. The orbit
// trap is a ring that follows the waveform, so treble draws bright
// filigree through the set where orbits brush past it.
//
// The dive is endless and exact. The repelling fixed point z* = z*² + c lies
// on the Julia set, and near it f(z) = z² + c acts like multiplication by
// λ = 2z*. Koenigs linearization makes that exact: ψ(u) = f^N(z* + u·λ^−N)
// (for large N) satisfies f(ψ(u)) = ψ(λu). Viewing ψ(u·λ^−dive) falls one
// level into the set as `dive` runs 0 → 1, and the picture at 1 is the
// picture at 0 with every escape count one higher. Subtracting `dive` from
// the count keeps the colors continuous, so the wrap leaves no seam. Since λ
// is complex, the fall turns as it goes: a spiral dive.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let c = vec2<f32>(q.cx, q.cy);

    let fix = julia_beta(c);
    // Keep it strictly repelling (λ → 1 at the cardioid's cusp).
    var lam = 2.0 * fix;
    lam = lam * (max(length(lam), 1.08) / max(length(lam), 1e-6));
    let log_lam = clog(lam);
    // Enough linearizing steps that u·λ^−N is tiny.
    let n_lin = i32(clamp(ceil(4.2 / log_lam.x), 4.0, 40.0));

    let view = rot2(q.spin) * centered(in.uv) * 1.5;
    var z = fix + cmul(view, cexp(-(q.dive + f32(n_lin)) * log_lam));
    for (var i = 0; i < n_lin; i = i + 1) {
        z = csq(z) + c;
    }

    let ring = 0.55 + waveform_at(atan2(z.y, z.x)) * 0.12;
    let j = julia(z, c, 96, ring);
    let n = j.x - q.dive;

    var col: vec3<f32>;
    if (j.z > 0.5) {
        // Outside: brightness climbs toward the boundary; hue cycles with depth.
        let glow = pow(clamp(n / 96.0 * 2.5, 0.0, 1.0), 2.2);
        let hue = cosine_palette(n * 0.11 + u.time * 0.012 + q.snap * 0.3, 0.35);
        // A faint field of color far out, rising to the bright rim, which
        // flares on the kick and dims when the kicks stop.
        let energy = 0.2 + 0.8 * q.groove * q.groove + q.punch * 1.4;
        col = hue * (0.0025 * (0.4 + 0.6 * q.groove) + glow * 0.045 * energy);
    } else {
        // Inside: deep indigo, with rose contour rings where orbits sink toward 0.
        let rings = pow(0.5 + 0.5 * cos(j.w * 40.0), 6.0) * smoothstep(0.8, 0.1, j.w);
        col = palette.colors[0].rgb * 0.004 + palette.colors[2].rgb * rings * 0.006;
    }

    // Orbit-trap filigree, brightened by treble and beats.
    let trap = exp(-j.y * 60.0) * (0.002 + q.trap * 0.02 + q.snap * 0.05);
    col = col + mix(palette.colors[1].rgb, palette.colors[3].rgb, 0.5) * trap;

    return vec4<f32>(col, 1.0);
}
