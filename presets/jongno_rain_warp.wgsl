// Jongno Rain warp — above the horizon, last frame lingers briefly so the
// rain streaks. Below it, the wet street mirrors the alley (water_reflect),
// and each of the last three kicks has dropped a ring into the puddles: it
// spreads, bends the reflection and catches the light as it fades.

const HORIZON_T: f32 = 0.72; // texture space (y down) of the composite's 0.28

@fragment
fn fs_warp(in: Varying) -> @location(0) vec4<f32> {
    let uv = in.uv;
    if (uv.y < HORIZON_T) {
        let src = warp_uv(uv);
        return finish(src, sample_prev(src));
    }

    var q = uv;
    var shine = 0.0;
    // Older rings are placed one beat apart: an estimate, since only the
    // newest kick's age is known.
    let interval = 60.0 / max(u.bpm, 60.0);
    // The kicks, or a Soba patch playing them
    let kick = drum(KICK);
    for (var k = 0; k < 3; k = k + 1) {
        let n = kick.count - f32(k);
        if (n < 1.0) {
            break;
        }
        let age = kick.age + f32(k) * interval;
        let center = vec2<f32>(
            0.08 + 0.84 * rand2(vec2<f32>(n, 11.0)),
            HORIZON_T + 0.03 + (0.97 - HORIZON_T - 0.03) * rand2(vec2<f32>(n, 12.0)));
        // Rings lie on the ground: flatter toward the horizon.
        let squash = 0.2 + 0.8 * (center.y - HORIZON_T) / (1.0 - HORIZON_T);
        let d = (q - center) * vec2<f32>(w.aspect, 1.0 / squash);
        let r = max(length(d), 1e-4);
        let x = r - age * 0.16;
        let band = exp(-x * x * 3000.0) * exp(-age * 2.0);
        let wave = sin(x * 220.0);
        let dir = d / r;
        q = q + vec2<f32>(dir.x / w.aspect, dir.y * squash) * wave * band * 0.004;
        shine = shine + max(wave, 0.0) * band;
    }

    let refl = water_reflect(q, HORIZON_T, 0.2 + u.bass * 0.2, u.mid * 2.0) * 0.85;
    let lit = mix(palette.colors[3].rgb, vec3<f32>(0.8, 0.86, 1.0), 0.5) * shine * 0.03;
    return vec4<f32>((refl + lit) * w.decay, 1.0);
}
