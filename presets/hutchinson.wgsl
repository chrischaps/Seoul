// Hutchinson seed: the warp builds the fractal; this only feeds it light.
// A waveform ring and a beat-lit core, both small and faint. Every copy of
// them the maps make is a copy of this.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let p = centered(in.uv);
    let r = length(p);
    let a = atan2(p.y, p.x);

    let ring_r = 0.30 + waveform_at(a) * 0.05;
    let ring = smoothstep(0.012, 0.0, abs(r - ring_r));
    // Snares flash the ring orchid; a hat sends a glint around it.
    // cv2 turns the ring's hue.
    let ramp = max(hue_rotate(palette_ramp(0.35 + 0.65 * spectrum_at(fract(a / TAU + 0.5))), u.cv[1] * PI), vec3<f32>(0.0));
    let ring_col = mix(ramp, palette.colors[3].rgb, q.snap);
    let hat = drum(HAT);
    let glint = pow(0.5 + 0.5 * cos(a - hat.age * 9.0), 24.0) * exp(-hat.age * 6.0);
    // cv1 lights the seed as the groove does.
    let lit = min(q.groove + max(u.cv[0], 0.0), 1.0);

    let core = smoothstep(0.06 + q.punch * 0.06, 0.0, r);

    var col = ring_col * ring * (0.015 + lit * 0.04 + q.snap * 0.08 + glint * 0.15);
    col = col + palette.colors[2].rgb * core * (0.01 + lit * 0.02 + q.punch * 0.16);
    return vec4<f32>(col, 1.0);
}
