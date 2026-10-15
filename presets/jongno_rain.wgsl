// Jongno Rain — a back alley in old Seoul on a wet night. Hanok roofs in
// the foreground, paper lattice doors glowing under their eaves; behind
// them, office blocks hung with vertical neon signs. Each sign is a VU
// meter for one fifth of the spectrum: its Hangul syllables light from the
// bottom up as the band gets louder. Hats brighten the rain and splash the
// street, snares flash the clouds (sometimes a bolt), and every kick drops
// a ripple into the puddles (see jongno_rain_warp.wgsl, which also mirrors
// everything into the wet street below the horizon).

const HORIZON: f32 = 0.28;
const SIGNS: f32 = 5.0;
const CELLS: f32 = 8.0;
const CELL: f32 = 0.046; // sign cell size, screen heights

// The initial consonant of a syllable, centered at `o`, half-size `h`:
// ㅇ ㄱ ㄴ ㅁ ㅅ ㅈ. Returns stroke distance.
fn consonant(q: vec2<f32>, o: vec2<f32>, h: f32, kind: u32) -> f32 {
    let a = q - o;
    let tl = vec2<f32>(-h, h);
    let tr = vec2<f32>(h, h);
    let bl = vec2<f32>(-h, -h);
    let br = vec2<f32>(h, -h);
    let apex = vec2<f32>(0.0, h);
    let mid = vec2<f32>(0.0, 0.1 * h);
    var d = 1e3;
    switch kind {
        case 0u: { d = abs(length(a) - h * 0.85); }
        case 1u: { d = min(sd_segment(a, tl, tr), sd_segment(a, tr, br)); }
        case 2u: { d = min(sd_segment(a, tl, bl), sd_segment(a, bl, br)); }
        case 3u: { d = abs(sd_box(a, vec2<f32>(h, h))); }
        case 4u: { d = min(sd_segment(a, apex, bl), sd_segment(a, mid, br)); }
        default: {
            d = min(sd_segment(a, tl, tr), min(sd_segment(a, apex, bl), sd_segment(a, mid, br)));
        }
    }
    return d;
}

// A Hangul-like syllable in a unit cell (q in -0.5..0.5): a consonant with
// a vertical vowel (ㅏ ㅓ ㅣ) beside it, sometimes over a final consonant,
// or a horizontal vowel (ㅗ ㅜ ㅡ) beneath it. Returns stroke distance.
fn syllable(q: vec2<f32>, seed: f32) -> f32 {
    let r0 = rand2(vec2<f32>(seed, 1.0));
    let r1 = rand2(vec2<f32>(seed, 2.0));
    let r2 = rand2(vec2<f32>(seed, 3.0));
    let r3 = rand2(vec2<f32>(seed, 4.0));
    let first = u32(r0 * 6.0);
    let tick = select(select(0.0, -0.13, r3 < 0.6), 0.13, r3 < 0.3); // ㅏ, ㅓ or ㅣ / ㅗ, ㅜ or ㅡ
    var d: f32;
    if (r1 < 0.65) {
        if (r2 < 0.4) {
            d = consonant(q, vec2<f32>(-0.12, 0.17), 0.11, first);
            d = min(d, sd_segment(q, vec2<f32>(0.2, -0.02), vec2<f32>(0.2, 0.38)));
            d = min(d, sd_segment(q, vec2<f32>(0.2, 0.18), vec2<f32>(0.2 + tick, 0.18)));
            let last = select(select(1u, 2u, r0 < 0.66), 0u, r0 < 0.33); // ㅇ ㄴ ㄱ
            d = min(d, consonant(q, vec2<f32>(0.0, -0.25), 0.11, last));
        } else {
            d = consonant(q, vec2<f32>(-0.13, 0.0), 0.15, first);
            d = min(d, sd_segment(q, vec2<f32>(0.22, -0.36), vec2<f32>(0.22, 0.36)));
            d = min(d, sd_segment(q, vec2<f32>(0.22, 0.0), vec2<f32>(0.22 + tick, 0.0)));
        }
    } else {
        d = consonant(q, vec2<f32>(0.0, 0.17), 0.13, first);
        d = min(d, sd_segment(q, vec2<f32>(-0.36, -0.12), vec2<f32>(0.36, -0.12)));
        d = min(d, sd_segment(q, vec2<f32>(0.0, -0.12), vec2<f32>(0.0, -0.12 + tick)));
    }
    return d;
}

// Neon tube color for sign `i`.
fn sign_color(i: f32) -> vec3<f32> {
    let k = u32(i) % 5u;
    let base = palette.colors[1u + k % 3u].rgb;
    return max(hue_rotate(base, select(0.0, 2.4, k >= 3u)), vec3<f32>(0.0));
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let p = in.uv;
    let c = centered(p);
    let t = u.time;

    // Below the horizon is the wet street, drawn by the warp; only the
    // splashes of the heaviest drops are added here.
    if (p.y < HORIZON) {
        let cell = floor(vec2<f32>(c.x * 260.0, p.y * 90.0));
        let splash = step(0.9985 - u.hat * 0.002, rand2(cell + floor(t * 24.0) * 7.31));
        return vec4<f32>(palette.colors[3].rgb * splash * 0.10, 1.0);
    }

    // Lightning: every snare lights the clouds; about a third throw a bolt.
    let strike = rand2(vec2<f32>(u.snare_count, 7.0)) < 0.33;
    let flash = u.snare * select(0.2, 1.0, strike);

    // Sky: city glow pooling at the horizon under slow low cloud.
    let haze = exp(-(p.y - HORIZON) * 5.0);
    var col = palette.colors[0].rgb * 0.008
        + mix(palette.colors[1].rgb, palette.colors[3].rgb, 0.6) * haze * 0.004;
    let cloud = smoothstep(0.45, 0.85, fbm(vec2<f32>(c.x * 1.4 + t * 0.012, p.y * 3.2 - t * 0.004)));
    col = col + cloud * (palette.colors[1].rgb * 0.0025 * haze + vec3<f32>(0.75, 0.82, 1.0) * flash * 0.05);
    col = col + vec3<f32>(0.75, 0.82, 1.0) * flash * 0.006;

    if (strike && u.snare_age < 0.15) {
        // A jagged bolt from the top of the frame to the rooftops.
        let x0 = (rand2(vec2<f32>(u.snare_count, 8.0)) - 0.5) * u.aspect * 0.8;
        var a = vec2<f32>(x0, 0.5);
        var bd = 1e3;
        for (var k = 1; k <= 8; k = k + 1) {
            let fk = f32(k);
            let b = vec2<f32>(
                a.x + (rand2(vec2<f32>(u.snare_count, 20.0 + fk)) - 0.5) * 0.09,
                0.5 - fk * 0.065);
            bd = min(bd, sd_segment(c, a, b));
            a = b;
        }
        let life = exp(-u.snare_age * 20.0);
        col = col + vec3<f32>(0.8, 0.86, 1.0) * (smoothstep(0.003, 0.0, bd) * 0.5 + glow(bd, 0.02) * 0.04) * life;
    }

    // Office blocks behind the alley, a few fluorescent windows still lit.
    let bcols = 9.0;
    let bi = floor(p.x * bcols);
    let btop = HORIZON + 0.22 + 0.30 * rand2(vec2<f32>(bi, 31.0));
    let in_block = step(p.y, btop) * step(0.015, fract(p.x * bcols)) * step(fract(p.x * bcols), 0.985);
    let wq = vec2<f32>(c.x * 110.0, p.y * 80.0);
    let wlit = step(0.93, rand2(floor(wq) + bi * 3.0))
        * step(0.45, fract(wq.x)) * step(0.5, fract(wq.y));
    let shade = 0.6 + 0.4 * rand2(vec2<f32>(bi, 32.0));
    col = mix(col, palette.colors[0].rgb * 0.005 * shade + vec3<f32>(0.75, 0.9, 1.0) * wlit * 0.005, in_block);

    // Vertical neon signs, one per slot: a VU meter of Hangul syllables.
    let si = floor(p.x * SIGNS);
    let sx = ((si + 0.3 + 0.4 * rand2(vec2<f32>(si, 41.0))) / SIGNS - 0.5) * u.aspect;
    let sbottom = HORIZON + 0.13 + 0.09 * rand2(vec2<f32>(si, 42.0)) - 0.5;
    let sl = vec2<f32>(c.x - sx, c.y - sbottom) / CELL; // sign space, cells
    let neon = sign_color(si);
    let board = sd_box(sl - vec2<f32>(0.0, CELLS * 0.5), vec2<f32>(0.62, CELLS * 0.5 + 0.12));
    if (board < 0.6) {
        let t0 = si / SIGNS;
        let level = (spectrum_at(t0 + 0.05) + spectrum_at(t0 + 0.1) + spectrum_at(t0 + 0.15)) / 3.0;
        let lit = level * CELLS * 1.15;
        // Backboard and its always-on outline tube.
        col = mix(col, palette.colors[0].rgb * 0.003, fill(board * CELL));
        col = col + neon * (smoothstep(0.05, 0.0, abs(board)) * 0.035 + glow(abs(board), 0.12) * 0.004);
        let j = floor(sl.y);
        if (j >= 0.0 && j < CELLS && abs(sl.x) < 0.5) {
            let q = vec2<f32>(sl.x, fract(sl.y) - 0.5) * 1.15;
            let d = syllable(q, si * 17.0 + j);
            let on = step(j + 0.5, lit);
            // The highest lit syllable sputters with the treble.
            let top = step(lit - 1.0, j) * on;
            let sputter = 1.0 - top * (0.5 * u.treble) * step(0.5, rand2(vec2<f32>(floor(t * 30.0), si)));
            let tube = smoothstep(0.045, 0.02, d);
            let halo = glow(d, 0.10);
            col = col + neon * (tube * (on * 0.09 * sputter + 0.004) + halo * on * 0.012 * sputter);
        }
    }

    // Hanok roofs across the foreground, lattice doors glowing beneath.
    let rslots = 3.0;
    let ri = floor(p.x * rslots + 0.25);
    let rcx = (ri - 0.25 + 0.5) / rslots;
    let xl = (p.x - rcx) * rslots * 2.0 / 0.98; // -1..1 across the roof
    let eave = HORIZON + 0.075 + 0.02 * rand2(vec2<f32>(ri, 51.0));
    let rise = 0.11 + 0.03 * rand2(vec2<f32>(ri, 52.0));
    let prof = hanok_roof(xl);
    if (prof >= 0.0) {
        let lift = smoothstep(0.7, 1.0, abs(xl));
        let top = eave + rise * prof;
        let under = eave - 0.012 + rise * 0.15 * lift * lift;
        if (p.y < top && p.y > under) {
            // Tile courses catch a little neon and every lightning flash.
            let courses = smoothstep(0.35, 0.0, abs(fract((top - p.y) * 150.0) - 0.5));
            let rim = smoothstep(0.003, 0.0, top - p.y) + smoothstep(0.002, 0.0, p.y - under) * 0.5;
            let sheen = mix(palette.colors[1].rgb, vec3<f32>(0.8, 0.86, 1.0), 0.5);
            col = palette.colors[0].rgb * 0.002
                + (palette.colors[1].rgb * 0.0012 + vec3<f32>(0.8, 0.86, 1.0) * flash * 0.01) * courses
                + sheen * rim * (0.012 + flash * 0.08);
        } else if (p.y <= under && abs(xl) < 0.66) {
            // Wall: three bays between posts above a stone footing, each
            // bay a hanji door with a changsal lattice, most of them lit.
            let bay = clamp(floor((xl + 0.66) / 1.32 * 3.0), 0.0, 2.0);
            let bx = fract((xl + 0.66) / 1.32 * 3.0) - 0.5;
            let door = step(abs(bx), 0.36) * step(HORIZON + 0.012, p.y) * step(p.y, under - 0.006);
            let lq = vec2<f32>(bx * 10.0, (p.y - HORIZON) * 190.0);
            let lattice = step(0.22, fract(lq.x)) * step(0.2, fract(lq.y));
            let door_on = step(0.35, rand2(vec2<f32>(ri, bay + 60.0)));
            let breathe = 0.85 + 0.15 * sin(t * 0.6 + ri * 2.0 + bay);
            col = palette.colors[0].rgb * 0.002
                + palette.colors[3].rgb * door * lattice * (door_on * breathe * 0.018 + 0.0008);
        }
    }

    // Rain over everything, lit by the street: hats make it glint.
    let r = rain(c, t, 0.45, 0.12);
    col = col + mix(vec3<f32>(0.7, 0.8, 1.0), palette.colors[3].rgb, 0.3) * r * (0.012 + u.hat * 0.03);

    return vec4<f32>(col, 1.0);
}
