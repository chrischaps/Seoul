// Kaleidoscope — fold theta into N sectors, draw a swirling stripe pattern.
// Sector count grows with bass for momentary "shatter" moments.
@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv - vec2<f32>(0.5);
    let r = length(p);
    let theta = atan2(p.y, p.x);

    let sectors = 6.0 + floor(u.bass * 4.0);
    let sector = TAU / sectors;
    // Mirror inside each sector — produces clean kaleidoscope reflections.
    let local = abs(theta - sector * floor(theta / sector + 0.5));

    // Stripe pattern that drifts with time
    let stripe_phase = local * 12.0 + r * 14.0 - u.time * 0.5;
    let stripe = step(0.55, fract(stripe_phase));

    // Pick color per stripe band
    let band = floor(stripe_phase * 0.5);
    let t = fract(band * 0.27);
    let col_a = mix(palette.colors[0].rgb, palette.colors[1].rgb, t);
    let col_b = mix(palette.colors[2].rgb, palette.colors[3].rgb, t);
    let col_pick = mix(col_a, col_b, step(0.5, fract(band * 0.5)));

    // Fade toward the rim so the center stays the focus
    let radial_fade = smoothstep(0.55, 0.05, r);

    let intensity = stripe * radial_fade * (0.025 + u.beat * 0.030);
    return vec4<f32>(col_pick * intensity, 1.0);
}
