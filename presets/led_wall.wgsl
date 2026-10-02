// LED Wall — the spectrum as a soft, mirrored blob: filled in a gradient
// of LED blues and violets, rimmed in pink-white like the panel photo in
// ref/. The post pass resamples the frame onto a round-dot LED grid; the
// mapping's vertical stretch makes old frames swell outward and fade, so
// the blob leaves concentric echoes.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    // Widen the spectrum's bass end, and smooth neighbors for a fluid edge.
    let x = pow(p.x, 0.8);
    let s = (spectrum_at(x - 0.012) + 2.0 * spectrum_at(x) + spectrum_at(x + 0.012)) * 0.25;
    let h = 0.03 + s * 0.40;
    let d = abs(p.y - 0.5);

    let inside = smoothstep(h, h - 0.012, d);
    let depth = clamp(d / max(h, 1e-3), 0.0, 1.0);
    let fill = palette_ramp(p.x) * inside * (0.016 + 0.026 * (1.0 - depth));
    let rim = smoothstep(0.010, 0.0, abs(d - h)) * (0.06 + u.beat * 0.06);

    return vec4<f32>(fill + palette.colors[3].rgb * rim, 1.0);
}
