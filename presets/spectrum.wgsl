// Spectrum — ports the Phase 2 spectrum mode. Bottom half is 64
// spectrum bars, top half is a waveform line. Black background lets
// additive blend preserve the warped feedback behind it.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    var col = vec3<f32>(0.0);

    if (uv.y < 0.5) {
        let idx_f = clamp(uv.x, 0.0, 0.9999) * 64.0;
        let idx = u32(idx_f);
        let mag = u.spectrum[idx];
        let bar_top = mag * 0.48;
        let frac = fract(idx_f);
        let in_bar = step(0.08, frac) * step(frac, 0.92);

        if (uv.y < bar_top && in_bar > 0.5) {
            let t = f32(idx) / 63.0;
            let bar_col = mix(palette.colors[0].rgb, palette.colors[1].rgb, t);
            let shade = 0.7 + 0.3 * (uv.y / max(bar_top, 1e-4));
            col = bar_col * shade * 0.10;
        }
    } else {
        let idx_f = clamp(uv.x, 0.0, 0.9999) * 512.0;
        let idx = u32(idx_f);
        let sample = u.waveform[idx];

        let y_target = 0.75 + sample * 0.18;
        let d = abs(uv.y - y_target);
        let line = smoothstep(0.006, 0.0, d);
        col = palette.colors[2].rgb * line * 0.22;
    }

    return vec4<f32>(col, 1.0);
}
