// Dancheong — the painted patterns on the eaves of Korean palaces and
// temples are built from concentric bands of petals and lotus scallops in
// four pigments. Here each ring is a scalloped band in one pigment; the
// rings advance one step per beat (driven by the tempo-locked beat_phase),
// so the pattern breathes outward in time with the music. The post pass
// folds it into eight mirrored wedges.

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let pc = polar(in.uv);
    let r = pc.x;
    let a = pc.y;

    let ring_w = 0.07;
    let rr = r / ring_w - u.beat_phase;
    let ring = floor(rr);
    let f = fract(rr);

    // Scalloped centerline: lobes alternate phase ring to ring.
    let lobes = 0.5 + 0.5 * cos(a * 8.0 + ring * PI);
    let line = abs(f - 0.5 - 0.28 * (lobes - 0.5));
    let band = smoothstep(0.07, 0.02, line);
    // A thin inner outline, as painted dancheong edges in white.
    let outline = smoothstep(0.015, 0.0, abs(line - 0.12));

    let ci = u32(abs(ring)) % 4u;
    let fade = smoothstep(0.75, 0.15, r);
    var col = palette.colors[ci].rgb * band * (0.08 + u.beat * 0.09);
    col = col + vec3<f32>(0.95, 0.93, 0.86) * outline * (0.02 + u.treble * 0.03);

    // A lotus at the center that opens with the bass.
    let lotus = smoothstep(0.05 + u.bass * 0.04, 0.0, r * (1.0 + 0.4 * lobes));
    col = col + palette.colors[3].rgb * lotus * 0.05;

    return vec4<f32>(col * fade, 1.0);
}
