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
        let pixels = self.read_rgba(device, queue)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        write_png(BufWriter::new(file), w, h, &pixels)
    }

    /// Copy the target to the CPU (blocking) as RGBA8, row by row.
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<u8>> {
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
        Ok(pixels)
    }
}

/// RGBA8 pixels as an sRGB PNG.
pub fn write_png(out: impl std::io::Write, w: u32, h: u32, pixels: &[u8]) -> Result<()> {
    let mut enc = png::Encoder::new(out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    // Recordings write thousands of frames; speed beats a few % of size.
    enc.set_compression(png::Compression::Fast);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    enc.write_header()?.write_image_data(pixels)?;
    Ok(())
}

/// Fill a `tw`×`th` picture from RGBA8 `w`×`h` pixels: crop the middle to
/// the new shape (a portrait frame gives its centre band to a landscape
/// card), then average each output pixel's patch of the source.
pub fn cover(pixels: &[u8], (w, h): (u32, u32), (tw, th): (u32, u32)) -> Vec<u8> {
    let (w, h, tw, th) = (w as usize, h as usize, tw as usize, th as usize);
    // The largest crop with the target's aspect ratio.
    let (cw, ch) = if w * th > h * tw { (h * tw / th, h) } else { (w, w * th / tw) };
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    // Output pixel `i` of `n` reads source pixels a..b along a crop of `c` from `o`.
    let span = |i: usize, n: usize, c: usize, o: usize| {
        let a = o + i * c / n;
        (a, (o + (i + 1) * c / n).max(a + 1))
    };
    let mut out = Vec::with_capacity(tw * th * 4);
    for ty in 0..th {
        let (ya, yb) = span(ty, th, ch, y0);
        for tx in 0..tw {
            let (xa, xb) = span(tx, tw, cw, x0);
            let mut sum = [0u32; 4];
            for y in ya..yb {
                for px in pixels[(y * w + xa) * 4..(y * w + xb) * 4].chunks_exact(4) {
                    for (s, &v) in sum.iter_mut().zip(px) {
                        *s += v as u32;
                    }
                }
            }
            let n = ((yb - ya) * (xb - xa)) as u32;
            out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portrait_frame_gives_its_middle_to_a_landscape_card() {
        // 4×8 portrait: rows 0-2 red, 3-4 green, 5-7 blue. A 4×2 card keeps
        // the middle two rows.
        let mut px = Vec::new();
        for y in 0..8 {
            let c = match y {
                0..3 => [255, 0, 0, 255],
                3..5 => [0, 255, 0, 255],
                _ => [0, 0, 255, 255],
            };
            for _ in 0..4 {
                px.extend(c);
            }
        }
        let out = cover(&px, (4, 8), (4, 2));
        assert_eq!(out.len(), 4 * 2 * 4);
        assert!(out.chunks_exact(4).all(|p| p == [0, 255, 0, 255]));
    }

    #[test]
    fn shrinking_averages_the_patch() {
        // 2×2 black and white checks shrink to one mid grey.
        let px = [0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255];
        assert_eq!(cover(&px, (2, 2), (1, 1)), vec![128, 128, 128, 255]);
        // The frame's 720×1280 makes a whole 640×360 card.
        let big = vec![7u8; 720 * 1280 * 4];
        assert_eq!(cover(&big, (720, 1280), (640, 360)).len(), 640 * 360 * 4);
    }
}
