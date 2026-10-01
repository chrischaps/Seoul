// Particle simulation: respawn the dead, advect the living through a curl
// noise flow field, with audio-coupled forces.

@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<uniform> p: ParticleUniforms;
@group(1) @binding(0) var<storage, read> u: AudioFeatures;
@group(2) @binding(0) var<uniform> palette: Palette;

fn spawn(i: u32) -> Particle {
    let seed = pcg(i * 9781u + p.frame * 6271u);
    let a = rnd(seed, 0u) * TAU;
    let dir = vec2<f32>(cos(a), sin(a));
    let half_w = 0.5 * p.aspect;

    var q: Particle;
    q.age = 0.0;
    q.life = p.life * (0.5 + rnd(seed, 1u));
    q.tag = rnd(seed, 2u);
    q.vel = dir * p.speed * (0.5 + rnd(seed, 3u));

    switch p.spawn {
        case SPAWN_RING: {
            q.pos = dir * (0.30 + 0.02 * (rnd(seed, 4u) - 0.5));
            q.tag = a / TAU;
        }
        case SPAWN_EDGES: {
            let t = rnd(seed, 4u);
            let side = u32(rnd(seed, 5u) * 4.0);
            switch side {
                case 0u: { q.pos = vec2<f32>(-half_w, t - 0.5); }
                case 1u: { q.pos = vec2<f32>(half_w, t - 0.5); }
                case 2u: { q.pos = vec2<f32>((t - 0.5) * 2.0 * half_w, -0.5); }
                default: { q.pos = vec2<f32>((t - 0.5) * 2.0 * half_w, 0.5); }
            }
            // Head inward.
            q.vel = -normalize(q.pos) * p.speed * (0.5 + rnd(seed, 3u));
        }
        case SPAWN_WAVEFORM: {
            let x = rnd(seed, 4u);
            q.pos = vec2<f32>((x - 0.5) * 2.0 * half_w, waveform_lin(x) * 0.3);
            q.tag = x;
        }
        case SPAWN_RANDOM: {
            q.pos = vec2<f32>((rnd(seed, 4u) - 0.5) * 2.0 * half_w, rnd(seed, 5u) - 0.5);
        }
        default: { // SPAWN_CENTER
            q.pos = dir * 0.03 * sqrt(rnd(seed, 4u));
        }
    }
    return q;
}

@compute @workgroup_size(256)
fn cs_update(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= p.count) {
        return;
    }
    var q = particles[i];
    q.age = q.age + p.dt;
    if (q.age >= q.life) {
        particles[i] = spawn(i);
        return;
    }

    let r = max(length(q.pos), 1e-4);
    let radial = q.pos / r;

    var acc = curl_noise(q.pos * p.flow_scale + vec2<f32>(p.time * 0.05, -p.time * 0.03)) * p.flow;
    acc = acc + radial * p.bass_push * u.bass;
    acc.y = acc.y - p.gravity;

    q.vel = q.vel + acc * p.dt;
    q.vel = q.vel * exp(-p.drag * p.dt);
    if (p.beat_trigger > 0.5) {
        let seed = pcg(i * 7919u + p.frame);
        q.vel = q.vel + radial * p.burst * (0.4 + rnd(seed, 7u));
    }
    q.pos = q.pos + q.vel * p.dt;
    particles[i] = q;
}
