// Vortex — spiral arms emanating from a hot core. Zoom < 1 in the mapping
// makes the feedback expand outward, pulling the spiral into long trails.
// Counter-rotation in the shader runs opposite to the mesh rotation so the
// arms appear to wind forever.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = centered(in.uv);
    let r = length(p);
    let theta = atan2(p.y, p.x);

    // Spiral arm pattern: arm count * theta, offset by log-radius and time.
    let arm_count = 5.0;
    let phase = arm_count * theta + r * 16.0 - u.time * 1.6;
    let arm = 0.5 + 0.5 * sin(phase);
    let arm_mask = smoothstep(0.65, 0.92, arm);

    // Radial envelope: bright core, soft fade to rim
    let core = smoothstep(0.08, 0.0, r);
    let mid_ring = smoothstep(0.55, 0.18, r);

    // Color goes from white-hot center → yellow → orange → deep red at edge
    let t = clamp(r * 2.1, 0.0, 1.0);
    let col_mid = mix(palette.colors[3].rgb, palette.colors[2].rgb, smoothstep(0.0, 0.35, t));
    let col_outer = mix(palette.colors[1].rgb, palette.colors[0].rgb, smoothstep(0.35, 1.0, t));
    let col_ramp = mix(col_mid, col_outer, smoothstep(0.3, 0.7, t));

    let arms = col_ramp * arm_mask * mid_ring;
    let blaze = palette.colors[3].rgb * core * (0.6 + u.beat * 0.6);

    let intensity = 0.040 + u.bass * 0.025 + u.beat * 0.030;
    return vec4<f32>((arms + blaze) * intensity, 1.0);
}
