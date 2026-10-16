// Ink — a soft drop of pale ink swells at the center with the bass and
// flares on beats; the custom warp (ink_warp.wgsl) carries it away on a
// curl-noise current. A cinnabar thread — the red of a painter's seal —
// traces the waveform around it.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);

    // The beat is the patch's gate 1 once it plays one (Soba link); cv1
    // swells the drop and cv2 tints the ink warm or cool.
    let q = params();
    let lift = max(u.cv[0], 0.0);
    let drop = exp(-r * r * (140.0 - 70.0 * u.bass - 40.0 * lift)) * (0.004 + q.pulse * 0.045 + lift * 0.01);
    let warm = mix(palette.colors[1].rgb, palette.colors[2].rgb, 0.4);
    let cool = palette.colors[1].rgb * vec3<f32>(0.6, 0.75, 1.0);
    let ink = mix(mix(palette.colors[1].rgb, warm, max(u.cv[1], 0.0)), cool, max(-u.cv[1], 0.0));
    var col = ink * drop;

    let theta = atan2(p.y, p.x);
    let ring_r = 0.27 + waveform_at(theta) * 0.05;
    let ring = smoothstep(0.004, 0.0, abs(r - ring_r));
    col = col + palette.colors[2].rgb * ring * (0.04 + u.treble * 0.08);

    return vec4<f32>(col, 1.0);
}
