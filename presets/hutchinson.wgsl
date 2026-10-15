// Hutchinson seed: the warp builds the fractal; this only feeds it light.
// A waveform ring and a beat-lit core, both small and faint. Every copy of
// them the maps make is a copy of this.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);
    let a = atan2(p.y, p.x);

    let ring_r = 0.30 + waveform_at(a) * 0.05;
    let ring = smoothstep(0.012, 0.0, abs(r - ring_r));
    let ring_col = palette_ramp(0.35 + 0.65 * spectrum_at(fract(a / TAU + 0.5)));

    let core = smoothstep(0.06 + u.bass * 0.04, 0.0, r);

    var col = ring_col * ring * (0.045 + u.volume * 0.045);
    col = col + palette.colors[2].rgb * core * (0.03 + u.beat * 0.09);
    return vec4<f32>(col, 1.0);
}
