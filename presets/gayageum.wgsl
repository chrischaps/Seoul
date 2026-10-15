// Gayageum: the twelve-string zither, played by a Soba patch over the link
// (docs/LINK.md). Each string has its own lane, so the patch is visible as
// an instrument.
//
// Strings 1–8 follow cv1–cv8. A lane's value slides that string's bridge
// (anjok) along the diagonal and presses the string beside it, as a
// player's left hand bends a note. Strings 9–12 belong to gate1–gate4: a
// rising edge plucks the string, and a gate held open keeps it humming. The
// silk cords that tie the strings off at the tail sway on the link clock.
//
// With no link, the music plays it instead. The spectrum stands in for the
// lanes, and kick, snare, hat and the beat pluck the four gate strings.

const STRINGS: i32 = 12;
const X_TAIL: f32 = -0.70; // strings leave the body here for their cords
const X_HEAD: f32 = 0.64;  // the ridge at the player's right
const TILT: f32 = -0.06;

// Offset of string `i` from the center line at `s` (0 tail → 1 head). The
// strings fan slightly wider toward the tail.
fn string_y(i: i32, s: f32) -> f32 {
    return (f32(i) - 5.5) * mix(0.027, 0.021, s);
}

// cv lane `i` (0–7), or the spectrum standing in when nothing is linked.
fn bend(i: i32) -> f32 {
    let heard = spectrum_at((f32(i) + 0.5) / 8.0) * 0.9;
    return mix(heard, u.cv[i], u.link_live);
}

// Seconds since gate string `g` (0–3) was plucked.
fn pluck_age(g: i32) -> f32 {
    if (u.link_live > 0.5) {
        return u.gate_age[g];
    }
    switch g {
        case 0: { return u.kick_age; }
        case 1: { return u.snare_age; }
        case 2: { return u.hat_age; }
        default: { return u.beat_phase * 60.0 / max(u.bpm, 1.0); }
    }
}

// Where string `i`'s bridge sits, as a fraction of its length.
fn bridge_at(i: i32) -> f32 {
    var s = 0.30 + 0.034 * f32(i);
    if (i < 8) {
        s = s + 0.07 * bend(i);
    }
    return s;
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let c0 = centered(in.uv) * 1.2;
    let cs = cos(TILT);
    let sn = sin(TILT);
    let c = vec2<f32>(c0.x * cs - c0.y * sn, c0.x * sn + c0.y * cs);
    let t = u.time;
    let px = 1.2 / max(u.resolution.y, 1.0);
    let wood_c = palette.colors[1].rgb;
    let silk = palette.colors[2].rgb;
    let cord_c = palette.colors[3].rgb;

    var col = palette.colors[0].rgb * 0.004;
    // A lamp somewhere above: a warm pool over the middle of the body.
    col = col + wood_c * 0.006 * glow(length(c * vec2<f32>(0.6, 1.0)) - 0.15, 0.35);

    // The body: a long paulownia slab, grain running along the strings.
    let body = sd_box(c - vec2<f32>(-0.03, 0.0), vec2<f32>(0.78, 0.17)) - 0.03;
    let grain = fbm(vec2<f32>(c.x * 1.6, c.y * 34.0 + fbm(c * 3.0) * 4.0));
    col = col + wood_c * (0.006 + 0.030 * grain * grain) * fill(body);
    col = col + wood_c * glow(abs(body), 0.0015) * 0.04;
    // Head and tail ridges the strings rest on.
    let head = sd_box(c - vec2<f32>(X_HEAD + 0.012, 0.0), vec2<f32>(0.010, 0.165));
    let tail = sd_box(c - vec2<f32>(X_TAIL - 0.010, 0.0), vec2<f32>(0.008, 0.180));
    col = col + silk * 0.03 * (fill(head) + fill(tail));

    // Bar position on the link clock (or the detected one), continuous
    // across beats, so the cords swing in time.
    let beats = u.beat_count + u.beat_phase;
    let s = (c.x - X_TAIL) / (X_HEAD - X_TAIL);
    let along = smoothstep(-0.004, 0.0, s) * smoothstep(1.004, 1.0, s);

    for (var i = 0; i < STRINGS; i = i + 1) {
        let sb = bridge_at(i);
        let y0 = string_y(i, s);
        var press = 0.0;
        var amp = 0.0;
        var bright = 0.25;
        var ring = 0.0;
        if (i < 8) {
            let v = bend(i);
            press = 0.010 * v;
            amp = 0.0012 * abs(v);
            bright = 0.25 + 0.7 * abs(v);
        } else {
            let g = i - 8;
            ring = exp(-pluck_age(g) * 2.2);
            let hold = u.gate[g];
            amp = 0.02 * ring + 0.0025 * hold;
            bright = 0.25 + 1.8 * ring + 0.5 * hold;
        }

        // Pressed left of the bridge; vibrating right of it, where it's plucked.
        var disp = 0.0;
        if (s < sb) {
            disp = -press * sin(PI * clamp(s / sb, 0.0, 1.0));
        } else {
            let ur = clamp((s - sb) / (1.0 - sb), 0.0, 1.0);
            disp = amp * sin(PI * ur) * sin(t * (61.0 + 7.0 * f32(i)) + f32(i) * 1.7);
        }
        let d = abs(c.y - y0 - disp);
        let core = smoothstep(1.6 * px, 0.0, d) * 0.10 + glow(d, 0.0035) * 0.03;
        col = col + silk * core * bright * along;
        // What the eye sees of a vibrating string: a soft lens between its
        // extremes, like a long exposure.
        if (s >= sb && amp > 0.0) {
            let env = amp * sin(PI * clamp((s - sb) / (1.0 - sb), 0.0, 1.0)) + px;
            let lens = smoothstep(env, env * 0.3, abs(c.y - y0));
            col = col + silk * lens * (ring * 0.11 + 0.015 * u.gate[i - 8]) * along;
        }
        // A plucked string lights the air around it.
        col = col + mix(silk, cord_c, 0.4) * glow(d, 0.02) * ring * 0.05 * along;

        // The bridge: a small ivory block under the string.
        let bx = mix(X_TAIL, X_HEAD, sb);
        let bd = sd_box(c - vec2<f32>(bx, string_y(i, sb)), vec2<f32>(0.0045, 0.0085)) - 0.0015;
        col = col + silk * (fill(bd) * 0.035 + glow(bd, 0.006) * 0.012 * bright);

        // The cord that ties it off, hanging past the tail and swaying.
        let tau = (X_TAIL - 0.02 - c.x) / 0.16;
        if (tau > 0.0 && tau < 1.0) {
            let phase = f32(i) * 0.37;
            let sway = 0.025 * sin(PI * beats * 0.5 + phase);
            let yc = string_y(i, 0.0) * (1.0 + 0.8 * tau) - 0.30 * tau * tau + sway * tau * tau;
            let slope = (string_y(i, 0.0) * 0.8 - 0.60 * tau + 2.0 * sway * tau) / -0.16;
            let dc = abs(c.y - yc) / sqrt(1.0 + slope * slope);
            col = col + cord_c * (smoothstep(2.0 * px, 0.0, dc) * 0.05 + glow(dc, 0.004) * 0.012)
                * (1.0 - tau * 0.5);
        }
    }

    return vec4<f32>(col, 1.0);
}
