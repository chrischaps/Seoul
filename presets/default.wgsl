// Default preset — ports the Phase 2 ink look.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);
    let theta = atan2(p.y, p.x);

    var col = vec3<f32>(0.0);

    // Pulsing ring
    let ring_r = 0.16 + u.bass * 0.14;
    let ring = smoothstep(0.014, 0.0, abs(r - ring_r));
    let ring_col = mix(palette.colors[0].rgb, palette.colors[1].rgb, u.mid);
    col = col + ring * ring_col * (0.20 + u.beat * 0.25);

    // Radial spokes on beat
    let spokes = abs(sin(theta * 8.0 + u.time * 0.3));
    let spoke_fade = smoothstep(0.45, 0.0, r);
    col = col + u.beat * spokes * spoke_fade * palette.colors[2].rgb * 0.10;

    // Waveform curl
    let sample = waveform_at(theta);
    let wave_r = 0.34 + sample * 0.08;
    let wave = smoothstep(0.008, 0.0, abs(r - wave_r));
    col = col + wave * mix(palette.colors[3].rgb, palette.colors[1].rgb, 0.55) * 0.14;

    return vec4<f32>(col, 1.0);
}
