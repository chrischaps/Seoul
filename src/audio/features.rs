use bytemuck::{Pod, Zeroable};

pub const SPECTRUM_BINS: usize = 64;
pub const WAVEFORM_SAMPLES: usize = 512;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct AudioFeatures {
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub volume: f32,
    pub beat: f32,
    pub time: f32,
    pub _pad: [f32; 2],
    pub spectrum: [f32; SPECTRUM_BINS],
    pub waveform: [f32; WAVEFORM_SAMPLES],
}

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            volume: 0.0,
            beat: 0.0,
            time: 0.0,
            _pad: [0.0; 2],
            spectrum: [0.0; SPECTRUM_BINS],
            waveform: [0.0; WAVEFORM_SAMPLES],
        }
    }
}
