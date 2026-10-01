// Transition mask: writes one preset's per-pixel weight into the feedback
// texture's alpha channel (alpha-only write mask). The following warp or
// composite draw blends with `src × DstAlpha`, so every preset gets
// dissolve/radial/clock transitions without knowing about them.

struct MaskUniforms {
    // Eased transition progress, 0..1.
    progress: f32,
    style: f32,
    // 0 = outgoing preset, 1 = incoming, 2 = no transition (solo).
    side: f32,
    // Extra factor (composite frame-time normalization).
    scale: f32,

    seed: f32,
    aspect: f32,
    _pad0: f32,
    _pad1: f32,
};
@group(0) @binding(0) var<uniform> m: MaskUniforms;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_mask(@builtin(vertex_index) vid: u32) -> Varying {
    let c = vec2<f32>(f32(vid & 1u) * 2.0, f32((vid >> 1u) & 1u) * 2.0);
    var out: Varying;
    out.pos = vec4<f32>(c * 2.0 - 1.0, 0.0, 1.0);
    out.uv = c;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    let h = vec2<u32>(vec2<i32>(floor(p))) * vec2<u32>(1597334673u, 3812015801u);
    return f32(((h.x ^ h.y) * 1597334673u) >> 8u) / 16777216.0;
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), s.x),
               mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), s.x), s.y);
}

// How much of the INCOMING preset shows at this pixel.
fn incoming(uv: vec2<f32>) -> f32 {
    let p = m.progress;
    let style = u32(m.style);
    let c = vec2<f32>((uv.x - 0.5) * m.aspect, uv.y - 0.5);
    let e = 0.08;
    // Stretch progress so the soft edge fully clears both ends.
    let q = p * (1.0 + 2.0 * e) - e;
    switch style {
        case 1u: { // dissolve
            let o = vec2<f32>(m.seed * 37.0, m.seed * 91.0);
            let n = 0.6 * vnoise(c * 5.0 + o) + 0.4 * vnoise(c * 13.0 + o);
            return smoothstep(n - e, n + e, q);
        }
        case 2u: { // radial
            let r = length(c) / (0.5 * sqrt(m.aspect * m.aspect + 1.0));
            return 1.0 - smoothstep(q - e, q + e, r);
        }
        case 3u: { // clock
            let a = fract(atan2(c.x, c.y) / 6.2831853 + m.seed);
            return 1.0 - smoothstep(q - e * 0.5, q + e * 0.5, a);
        }
        default: { // crossfade, zoom
            return p;
        }
    }
}

@fragment
fn fs_mask(in: Varying) -> @location(0) vec4<f32> {
    var w = 1.0;
    if (m.side < 0.5) {
        w = 1.0 - incoming(in.uv);
    } else if (m.side < 1.5) {
        w = incoming(in.uv);
    }
    return vec4<f32>(0.0, 0.0, 0.0, w * m.scale);
}
