// HUD rectangles: instanced rounded rects with an SDF edge, in pixels,
// blended premultiplied-over the finished frame.

struct Screen {
    size: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> screen: Screen;

struct RectIn {
    // x, y, width, height in pixels (top-left origin).
    @location(0) rect: vec4<f32>,
    // Linear RGB + alpha (not premultiplied).
    @location(1) color: vec4<f32>,
    // corner radius, edge softness.
    @location(2) style: vec2<f32>,
};

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) style: vec2<f32>,
};

const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_rect(@builtin(vertex_index) vid: u32, r: RectIn) -> Varying {
    let c = CORNERS[vid];
    // Pad by the softness so the anti-aliased edge isn't clipped.
    let pad = r.style.y + 1.0;
    let px = r.rect.xy - pad + c * (r.rect.zw + 2.0 * pad);
    var out: Varying;
    out.pos = vec4<f32>(px / screen.size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.half_size = r.rect.zw * 0.5;
    out.local = px - (r.rect.xy + out.half_size);
    out.color = r.color;
    out.style = r.style;
    return out;
}

@fragment
fn fs_rect(in: Varying) -> @location(0) vec4<f32> {
    let radius = min(in.style.x, min(in.half_size.x, in.half_size.y));
    let q = abs(in.local) - in.half_size + radius;
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let aa = max(in.style.y, 0.75);
    let a = in.color.a * (1.0 - smoothstep(-aa, aa, d));
    return vec4<f32>(in.color.rgb * a, a);
}
