// Plasma — sinusoidal color fields that drift with time, beat,
// and a hint of spectrum. The heavy warp in the mapping smears this
// into flowing psychedelic shapes over many frames.
//
// IMPORTANT: this shader paints every pixel each frame. Composite uses
// additive blend onto the warped feedback, so the per-pixel contribution
// must stay tiny — steady state brightness is roughly C / (1 - decay) ≈ 33×C
// for decay 0.97. Anything above ~0.03 per channel per frame saturates to
// pure white. We average the three field colors and scale down to ~0.03.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = (in.uv - vec2<f32>(0.5)) * 2.0; // -1..1
    let r = length(p);

    let t = u.time;

    // Three interfering sinusoidal fields, phase-shifted, audio-modulated
    let f0 = sin(p.x * 6.0 + t * 0.7);
    let f1 = cos(p.y * 5.0 + t * 0.5 + u.bass * 2.0);
    let f2 = sin((p.x + p.y) * 4.0 + t * 0.3 + u.mid * 3.0);

    let w0 = 0.5 + 0.5 * f0;
    let w1 = 0.5 + 0.5 * f1;
    let w2 = 0.5 + 0.5 * f2;

    // Average (not sum) the three palette contributions, then scale way down.
    let field = (palette.colors[0].rgb * w0
               + palette.colors[1].rgb * w1
               + palette.colors[2].rgb * w2) * (1.0 / 3.0);
    var col = field * (0.020 + u.beat * 0.025);

    // Sparse bright center that swells with bass
    let core = smoothstep(0.20 + u.bass * 0.10, 0.0, r);
    col = col + core * palette.colors[3].rgb * 0.025;

    return vec4<f32>(col, 1.0);
}
