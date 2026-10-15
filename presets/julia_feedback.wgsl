// Feedback Julia seed: a small glowing disk at the origin. Through the warp,
// every point sums the light its own orbit passes over before it escapes,
// so the Julia set appears shaded by how often its orbits return near 0
// (an orbit trap, computed by the feedback loop with no loop in the shader).

const SCALE: f32 = 2.9;

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let z = centered(in.uv) * SCALE;
    let r = length(z);
    let a = atan2(z.y, z.x);

    // Concentric hues: orbits that pass at different depths through the disk
    // carry different colors, which lands as rings in every preimage.
    let hue = palette_ramp(0.5 + 0.5 * sin(r * 14.0 - u.time * 0.6));
    let disk = smoothstep(0.32 + u.bass * 0.08, 0.0, r);

    // The waveform ring adds a thread of brighter light into every orbit.
    let wave = smoothstep(0.025, 0.0, abs(r - 0.9 - waveform_at(a) * 0.15));

    let col = hue * disk * (0.04 + u.beat * 0.05) + palette.colors[2].rgb * wave * 0.02;
    return vec4<f32>(col, 1.0);
}
