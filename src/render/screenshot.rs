//! Offscreen capture of the final frame to PNG.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// A render target with the swapchain's format, so the regular post
/// pipeline can draw into it unchanged.
pub struct CaptureTarget {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

impl CaptureTarget {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("seoul.screenshot"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            size,
            format,
        }
    }

    /// Copy the target to the CPU (blocking) and write it as an sRGB PNG.
    pub fn save_png(&self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) -> Result<()> {
        let (w, h) = self.size;
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("seoul.screenshot.readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("seoul.screenshot.copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("device poll failed: {e}"))?;
        rx.recv()
            .context("map callback dropped")?
            .map_err(|e| anyhow!("buffer map failed: {e}"))?;

        let bgra = matches!(
            self.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut pixels = Vec::with_capacity((unpadded * h) as usize);
        {
            let data = slice
                .get_mapped_range()
                .map_err(|e| anyhow!("mapped range unavailable: {e}"))?;
            for row in data.chunks(padded as usize) {
                for px in row[..unpadded as usize].chunks_exact(4) {
                    if bgra {
                        pixels.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    } else {
                        pixels.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
            }
        }
        buffer.unmap();

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // Recordings write thousands of frames; speed beats a few % of size.
        enc.set_compression(png::Compression::Fast);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        enc.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}

/// `screenshots/seoul-<preset>-<unix-millis>.png`
pub fn default_path(preset: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let slug: String = preset
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    PathBuf::from("screenshots").join(format!("seoul-{slug}-{stamp}.png"))
}
