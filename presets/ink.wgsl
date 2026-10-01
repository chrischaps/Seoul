// Ink — a soft drop of pale ink swells at the center with the bass and
// flares on beats; the custom warp (ink_warp.wgsl) carries it away on a
// curl-noise current. A cinnabar thread — the red of a painter's seal —
// traces the waveform around it.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);

    let drop = exp(-r * r * (110.0 - 60.0 * u.bass)) * (0.015 + u.beat * 0.10);
    var col = palette.colors[1].rgb * drop;

    let theta = atan2(p.y, p.x);
    let ring_r = 0.27 + waveform_at(theta) * 0.05;
    let ring = smoothstep(0.004, 0.0, abs(r - ring_r));
    col = col + palette.colors[2].rgb * ring * (0.04 + u.treble * 0.08);

    return vec4<f32>(col, 1.0);
}
