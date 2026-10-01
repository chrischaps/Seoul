struct BlitUniforms {
    resolution: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: BlitUniforms;
@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

const SOURCE_ASPECT: f32 = 16.0 / 9.0;

@vertex
fn vs_blit(@builtin(vertex_index) vid: u32) -> Varying {
    let uv = vec2<f32>(
        f32(vid & 1u) * 2.0,
        f32((vid >> 1u) & 1u) * 2.0,
    );
    var out: Varying;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    // Flip Y so uv.y=0 means top of window (matches textureSample convention).
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_blit(in: Varying) -> @location(0) vec4<f32> {
    let win_aspect = u.resolution.x / max(u.resolution.y, 1.0);
    var p = in.uv - vec2<f32>(0.5);

    if (win_aspect > SOURCE_ASPECT) {
        // Window wider than source: pillarbox (scale x out so source fits height)
        p.x = p.x * (win_aspect / SOURCE_ASPECT);
    } else {
        // Window taller than source: letterbox
        p.y = p.y * (SOURCE_ASPECT / win_aspect);
    }

    let src_uv = p + vec2<f32>(0.5);
    if (src_uv.x < 0.0 || src_uv.x > 1.0 || src_uv.y < 0.0 || src_uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return textureSample(src, samp, src_uv);
}
