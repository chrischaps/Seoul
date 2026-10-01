use bytemuck::{Pod, Zeroable};

pub const SPECTRUM_BINS: usize = 64;
pub const WAVEFORM_SAMPLES: usize = 512;

/// Per-frame audio + timing snapshot.
///
/// This is both the CPU-side value the analysis thread publishes and the
/// exact GPU storage-buffer layout bound at `@group(0) @binding(0)` in every
/// preset shader. Field order and padding must match the `AudioFeatures`
/// struct in `shaders/composite_prelude.wgsl` exactly — every scalar is f32
/// and `resolution` sits on a 16-byte boundary so std430 and `repr(C)` agree.
///
/// Audio fields are written by the analysis thread. Timing fields (`time`,
/// `dt`, `frame`, `aspect`, `resolution`) are stamped by the render thread
/// just before upload, so visuals keep moving even when no audio arrives.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct AudioFeatures {
    // Instantaneous band levels, 0..1, per-band auto-gained.
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub volume: f32,

    // ~1 s attenuated band averages (MilkDrop `*_att` semantics).
    pub bass_att: f32,
    pub mid_att: f32,
    pub treble_att: f32,
    /// Onset envelope: jumps to 1 on a detected beat, decays over ~100 ms.
    pub beat: f32,

    // Render-thread clock.
    pub time: f32,
    pub dt: f32,
    pub frame: f32,
    /// Tempo estimate in BPM (120 until enough beats have been heard).
    pub bpm: f32,

    /// 0→1 sawtooth locked to the estimated tempo; wraps on each beat.
    pub beat_phase: f32,
    /// Count of detected beats since start.
    pub beat_count: f32,
    /// Width / height of the render target.
    pub aspect: f32,
    /// 0..1 confidence in the tempo estimate.
    pub bpm_confidence: f32,

    pub resolution: [f32; 2],
    pub _pad: [f32; 2],

    /// Log-spaced spectrum, 0..1, perceptually tilted.
    pub spectrum: [f32; SPECTRUM_BINS],
    /// Trigger-aligned, auto-gained waveform, roughly -1..1, ends tapered to 0.
    pub waveform: [f32; WAVEFORM_SAMPLES],
}

// Header (20 scalars) + spectrum + waveform. If this fails, the WGSL prelude
// is out of sync with the Rust struct.
const _: () = assert!(
    std::mem::size_of::<AudioFeatures>() == (20 + SPECTRUM_BINS + WAVEFORM_SAMPLES) * 4
);

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            aspect: 16.0 / 9.0,
            resolution: [1280.0, 720.0],
            ..Zeroable::zeroed()
        }
    }
}
