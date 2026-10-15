//! Spectrum history: the last few seconds of `AudioFeatures::spectrum` as a
//! scrolling spectrogram texture, so presets can draw the recent past
//! (ridgelines that recede, waves that roll in).
//!
//! A `SPECTRUM_BINS × ROWS` ring of `R32Float`, written by the render thread
//! at a fixed `ROWS_PER_SECOND` of render time (frame-rate independent, and
//! deterministic under `--record`). `AudioFeatures::hist_pos` tells shaders
//! where the ring's head is, including how far it is toward the next row so
//! history scrolls smoothly; `spectrum_ago` in `shaders/common.wgsl` does the
//! lookup.

use crate::audio::{AudioFeatures, features::SPECTRUM_BINS};

pub const ROWS: u32 = 256;
pub const ROWS_PER_SECOND: f32 = 30.0;
/// Never write more than this many rows in one frame (after a stall).
const MAX_CATCH_UP: u32 = 8;

pub struct SpectrumHistory {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Rows written so far.
    written: u64,
    /// Render time the next row is due.
    next_due: f32,
}

impl SpectrumHistory {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("seoul.spectrum_history"),
            size: wgpu::Extent3d { width: SPECTRUM_BINS as u32, height: ROWS, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, written: 0, next_due: 0.0 }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The bind group layout entry (non-filterable float, read with
    /// `textureLoad`) for `binding`.
    pub fn layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }
    }

    /// Append rows due by `f.time` and stamp `f.hist_pos`.
    pub fn update(&mut self, queue: &wgpu::Queue, f: &mut AudioFeatures) {
        if f.time < self.next_due - 1.0 {
            // The clock went backwards (a new --record run): start over.
            self.next_due = f.time;
        }
        let mut rows = 0;
        while f.time >= self.next_due && rows < MAX_CATCH_UP {
            let row = (self.written % ROWS as u64) as u32;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&f.spectrum),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SPECTRUM_BINS as u32 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d { width: SPECTRUM_BINS as u32, height: 1, depth_or_array_layers: 1 },
            );
            self.written += 1;
            self.next_due += 1.0 / ROWS_PER_SECOND;
            rows += 1;
        }
        if f.time >= self.next_due {
            // Too far behind to catch up row by row; skip ahead.
            self.next_due = f.time + 1.0 / ROWS_PER_SECOND;
        }
        f.hist_pos = hist_pos(self.written, self.next_due, f.time);
    }
}

/// The ring head as a float: the newest row's index (mod `ROWS`) plus how
/// far render time has moved toward the next row.
fn hist_pos(written: u64, next_due: f32, time: f32) -> f32 {
    if written == 0 {
        return 0.0;
    }
    let newest = ((written - 1) % ROWS as u64) as f32;
    let frac = (1.0 - (next_due - time) * ROWS_PER_SECOND).clamp(0.0, 0.999);
    newest + frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_wraps_and_advances_between_rows() {
        let dt = 1.0 / ROWS_PER_SECOND;
        // Row 0 was just written at t = 0; the next is due at dt.
        assert_eq!(hist_pos(1, dt, 0.0), 0.0);
        assert!((hist_pos(1, dt, dt * 0.5) - 0.5).abs() < 1e-4);
        // The ring wraps: the 257th row lands in slot 0 again.
        assert_eq!(hist_pos(ROWS as u64 + 1, dt, 0.0), 0.0);
        assert_eq!(hist_pos(ROWS as u64, dt, 0.0), (ROWS - 1) as f32);
    }
}
