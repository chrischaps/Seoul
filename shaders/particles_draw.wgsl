// Particle drawing: one soft round sprite per particle, added straight into
// the feedback texture so the warp turns them into trails.

@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<uniform> p: ParticleUniforms;
@group(1) @binding(0) var<storage, read> u: AudioFeatures;
@group(2) @binding(0) var<uniform> palette: Palette;

struct SpriteOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec3<f32>,
};

const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
);

@vertex
fn vs_particle(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> SpriteOut {
    let q = particles[iid];
    let corner = CORNERS[vid];
    var out: SpriteOut;
    out.local = corner;

    // Fade in and out over the particle's life; dead ones collapse to nothing.
    let t = clamp(q.age / max(q.life, 1e-3), 0.0, 1.0);
    let env = sin(PI * t);

    var color: vec3<f32>;
    switch p.color_mode {
        case COLOR_RAMP: { color = palette_ramp(t); }
        case COLOR_SPECTRUM: { color = palette_ramp(q.tag) * (0.4 + 1.6 * spectrum_at(q.tag)); }
        default: { color = palette.colors[min(p.color_index, 3u)].rgb; }
    }
    out.color = color * env * p.intensity;

    let size_px = p.size * p.resolution.y / 1080.0 * (0.4 + 0.6 * env);
    let ndc = vec2<f32>(q.pos.x * 2.0 / p.aspect, q.pos.y * 2.0);
    let offset = corner * size_px / p.resolution * 2.0;
    out.pos = vec4<f32>(ndc + offset, 0.0, 1.0);
    return out;
}

@fragment
fn fs_particle(in: SpriteOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.local, in.local);
    let a = exp(-r2 * 4.0) * (1.0 - smoothstep(0.7, 1.0, r2));
    return vec4<f32>(in.color * a, 1.0);
}
