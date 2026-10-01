// Shared by the particle simulation (compute) and drawing (render) shaders.
// Particle positions live in the same centered, aspect-correct space as
// `centered(uv)`: y in -0.5..0.5 (up), x in ±0.5·aspect.

struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
    age: f32,
    life: f32,
    // 0..1 tag picked at spawn — drives "spectrum" coloring.
    tag: f32,
    _pad: f32,
};

// Layout matches Rust `ParticleUniforms` (6 × vec4 = 96 bytes).
struct ParticleUniforms {
    dt: f32,
    time: f32,
    count: u32,
    spawn: u32,

    speed: f32,
    flow: f32,
    flow_scale: f32,
    drag: f32,

    size: f32,
    life: f32,
    color_mode: u32,
    color_index: u32,

    burst: f32,
    bass_push: f32,
    gravity: f32,
    intensity: f32,

    beat_trigger: f32,
    aspect: f32,
    resolution: vec2<f32>,

    frame: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

const SPAWN_CENTER: u32 = 0u;
const SPAWN_RING: u32 = 1u;
const SPAWN_EDGES: u32 = 2u;
const SPAWN_WAVEFORM: u32 = 3u;
const SPAWN_RANDOM: u32 = 4u;

const COLOR_PALETTE: u32 = 0u;
const COLOR_RAMP: u32 = 1u;
const COLOR_SPECTRUM: u32 = 2u;

fn pcg(v_in: u32) -> u32 {
    let state = v_in * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Per-particle random stream: rnd(seed, k) for k = 0, 1, 2, …
fn rnd(seed: u32, k: u32) -> f32 {
    return f32(pcg(seed ^ pcg(k + 0x9e3779b9u))) / 4294967295.0;
}
