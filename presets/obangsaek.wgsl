// Obangsaek: the five cardinal colors (blue east, red south, yellow center,
// white west, black north) on a baramgaebi, the paper pinwheel children run
// with. A link preset (docs/LINK.md).
//
// The angle is read straight off the clock, so with a Soba link the wheel
// turns exactly once per bar, locked to the patch. cv1–cv5 feed the five
// colors: each blade grows and brightens with its lane, and cv3 swells the
// yellow pin at the center. Each gate throws a five-lobed bloom of color
// outward, and the feedback carries it off like wind.

// The five colors, in cv order: blue, red, yellow, white, black.
const OBANG = array<vec3<f32>, 5>(
    vec3<f32>(0.10, 0.32, 0.90),
    vec3<f32>(0.95, 0.16, 0.12),
    vec3<f32>(1.00, 0.78, 0.20),
    vec3<f32>(0.96, 0.94, 0.88),
    vec3<f32>(0.10, 0.10, 0.16),
);
// Blade k (around the wheel) wears color BLADE[k]: blue, red, white, black.
const BLADE = array<i32, 4>(0, 1, 3, 4);
// Gate g throws color BLOOM[g]: red, blue, white, yellow.
const BLOOM = array<i32, 4>(1, 0, 3, 2);
const CENTER = vec2<f32>(0.0, 0.04);
const R: f32 = 0.22;

// Weight 0..1 of color `k`, from its cv lane or, without a link, the music.
fn weight(k: i32) -> f32 {
    var heard = 0.0;
    switch k {
        case 0: { heard = u.treble_att; }
        case 1: { heard = u.bass_att; }
        case 2: { heard = u.volume; }
        case 3: { heard = u.mid_att; }
        default: { heard = 0.5 + 0.5 * u.beat; }
    }
    return clamp(mix(heard, abs(u.cv[k]), u.link_live), 0.0, 1.0);
}

// Seconds since gate `g` opened; without a link, the drums and the downbeat.
fn bloom_age(g: i32) -> f32 {
    if (u.link_live > 0.5) {
        return u.gate_age[g];
    }
    switch g {
        case 0: { return u.kick_age; }
        case 1: { return u.snare_age; }
        case 2: { return u.hat_age; }
        default: {
            let beat_len = 60.0 / max(u.bpm, 1.0);
            return (u.beat_count % 4.0 + u.beat_phase) * beat_len;
        }
    }
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let c = centered(in.uv);
    let q = c - CENTER;
    let r = length(q);
    let th = atan2(q.y, q.x);

    var col = palette.colors[0].rgb * 0.002;

    // The stick, leaning down to the right from the pin.
    let stick = sd_segment(c, CENTER, vec2<f32>(0.10, -0.55)) - 0.006;
    col = col + palette.colors[3].rgb * (fill(stick) * 0.006 + glow(stick, 0.003) * 0.001);

    // Once around per bar, clockwise, on whichever clock is in charge.
    let bar = fract((u.beat_count + u.beat_phase) / 4.0);
    let spin = -bar * TAU;

    // Four curled blades. `a` runs 0 → 1 across each blade from its leading
    // edge, where the paper is widest, to the fold that curls into the next.
    let sector = (th - spin) / (TAU / 4.0);
    let k = i32(floor(sector)) & 3;
    let a = fract(sector);
    let w = weight(BLADE[k]);
    let reach = (R * (0.8 + 0.35 * w)) * pow(1.0 - a, 0.55);
    let bd = r - reach;
    let paper = OBANG[BLADE[k]];
    // The fold catches the light; the leading edge is in shade.
    let shade = mix(0.55, 1.15, a) * (0.35 + 0.9 * w);
    var blade = paper * shade * 0.12 * fill(bd);
    // Black paper shows as its silver rim and a dim sheen.
    if (BLADE[k] == 4) {
        blade = blade + vec3<f32>(0.55, 0.6, 0.75) * glow(abs(bd), 0.003) * 0.05 * (0.4 + w);
    }
    col = col + blade + paper * glow(bd, 0.02) * 0.006 * (0.3 + w);
    // Crease lines where each blade meets the next.
    let crease = abs(fract(sector + 0.5) - 0.5) * r * TAU / 4.0;
    col = col + palette.colors[1].rgb * smoothstep(0.002, 0.0, crease) * fill(bd) * 0.008;

    // The yellow pin at the center (cv3: the earth at the middle).
    let wy = weight(2);
    let pin = r - (0.018 + 0.012 * wy);
    col = col + OBANG[2] * (fill(pin) * (0.10 + 0.12 * wy) + glow(pin, 0.03) * 0.03 * wy);

    // Blooms: a five-lobed ring per gate, one lobe for each direction.
    for (var g = 0; g < 4; g = g + 1) {
        let age = bloom_age(g);
        let fade = exp(-age * 1.8);
        if (fade > 0.01) {
            let rr = R * 1.15 + age * 0.45;
            let lobes = 0.012 * sin(5.0 * (th - spin) + f32(g) * 1.3);
            let d = abs(r - rr - lobes);
            col = col + OBANG[BLOOM[g]] * (smoothstep(0.006, 0.0, d) * 0.08 + glow(d, 0.02) * 0.02) * fade * select(1.0, 0.45, g == 2);
        }
    }

    return vec4<f32>(col, 1.0);
}
