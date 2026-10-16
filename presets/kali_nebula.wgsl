// Starfield of Kali: the 2D Kali set. How much |p| changes along the orbit
// gives the nebula's density; how close the orbit dips toward the origin
// gives pinpoint stars.
//
// Flight: three copies of the field at staggered depths, each zooming in
// by ZOOM per cycle and fading in from far away, swelling past, and fading
// out, a third of a cycle apart (sin² weights that always sum to 1.5). Each
// copy gets a fresh offset whenever it restarts, so the stars you fly
// through never repeat.

const ZOOM: f32 = 5.0;
const LAYERS: i32 = 3;

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let q = params();
    let view = rot2(q.turn) * centered(in.uv) * q.depth;
    let c = vec2<f32>(q.cx, q.cy);

    var gas = vec3<f32>(0.0);
    var stars = 0.0;
    for (var i = 0; i < LAYERS; i = i + 1) {
        let t = q.fly + f32(i) / f32(LAYERS);
        let phase = fract(t);
        let cycle = floor(t) * 3.0 + f32(i);
        let weight = pow(sin(phase * PI), 2.0) / 1.5;
        let offset = vec2<f32>(rand2(vec2<f32>(cycle, 1.0)), rand2(vec2<f32>(cycle, 7.0))) * 2.0 - 1.0;
        let p = view / pow(ZOOM, phase) + offset * 0.6;

        let k = kali(p, c, 14);
        let density = k.x / 14.0;
        gas = gas + cosine_palette(density * 0.6 + u.time * 0.006 + q.snap * 0.25 + u.cv[1] * 0.4, 0.15) * pow(density, 2.2) * weight;
        stars = stars + exp(-k.y * 28.0) * weight;
    }

    let glow = 0.3 + 0.5 * q.groove + q.punch * 1.6 + max(u.cv[0], 0.0) * 0.6;
    let col = gas * 0.010 * glow + palette.colors[3].rgb * stars * (0.004 + drum(HAT).hit * 0.03 + q.punch * 0.02);
    return vec4<f32>(col, 1.0);
}
