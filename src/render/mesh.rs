use bytemuck::{Pod, Zeroable};
use tracing::info;
use wgpu::util::DeviceExt;

pub const GRID_COLS: u32 = 48;
pub const GRID_ROWS: u32 = 36;
pub const VERTEX_COUNT: u32 = GRID_COLS * GRID_ROWS;
pub const INDEX_COUNT: u32 = (GRID_COLS - 1) * (GRID_ROWS - 1) * 6;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct WarpVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
}

/// Per-frame warp parameters evaluated from the active preset.
#[derive(Copy, Clone, Debug, Default)]
pub struct WarpParams {
    pub zoom: f32,
    pub rotation: f32,
    pub warp_amount: f32,
}

pub struct WarpMesh {
    pub vertices: Vec<WarpVertex>,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    pub index_count: u32,
}

impl WarpMesh {
    pub fn new(device: &wgpu::Device) -> Self {
        let vertices = identity_grid();
        let indices = grid_indices();

        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.warp_mesh.vbuf"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("seoul.warp_mesh.ibuf"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        info!(
            vertices = vertices.len(),
            indices = indices.len(),
            "warp mesh built"
        );

        Self {
            vertices,
            vbuf,
            ibuf,
            index_count: indices.len() as u32,
        }
    }

    pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WarpVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as u64,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }

    /// Recompute UVs from per-frame warp parameters. Positions are fixed at
    /// grid locations.
    pub fn update(&mut self, params: &WarpParams, time: f32) {
        let zoom = params.zoom.max(0.001); // avoid divide by zero
        let (sin_r, cos_r) = params.rotation.sin_cos();

        for (i, vertex) in self.vertices.iter_mut().enumerate() {
            let col = (i as u32) % GRID_COLS;
            let row = (i as u32) / GRID_COLS;
            let u0 = col as f32 / (GRID_COLS - 1) as f32;
            let v0 = row as f32 / (GRID_ROWS - 1) as f32;

            // Flip V for texture sampling (UV origin top-left, clip Y up)
            let u_src = u0;
            let v_src = 1.0 - v0;

            let cx = u_src - 0.5;
            let cy = v_src - 0.5;
            let zx = cx / zoom;
            let zy = cy / zoom;
            let rx = cos_r * zx - sin_r * zy;
            let ry = sin_r * zx + cos_r * zy;

            let px = vertex.pos[0];
            let py = vertex.pos[1];
            let wx = params.warp_amount * (py * 3.0 + time * 0.5).sin();
            let wy = params.warp_amount * (px * 3.0 + time * 0.7).cos();

            vertex.uv[0] = rx + 0.5 + wx;
            vertex.uv[1] = ry + 0.5 + wy;
        }
    }

    pub fn upload(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&self.vertices));
    }
}

fn identity_grid() -> Vec<WarpVertex> {
    let mut verts = Vec::with_capacity(VERTEX_COUNT as usize);
    for row in 0..GRID_ROWS {
        for col in 0..GRID_COLS {
            let u = col as f32 / (GRID_COLS - 1) as f32;
            let v = row as f32 / (GRID_ROWS - 1) as f32;
            let x = u * 2.0 - 1.0;
            let y = v * 2.0 - 1.0;
            verts.push(WarpVertex {
                pos: [x, y],
                uv: [u, 1.0 - v],
            });
        }
    }
    verts
}

fn grid_indices() -> Vec<u16> {
    let mut indices = Vec::with_capacity(INDEX_COUNT as usize);
    for row in 0..(GRID_ROWS - 1) {
        for col in 0..(GRID_COLS - 1) {
            let i0 = (row * GRID_COLS + col) as u16;
            let i1 = i0 + 1;
            let i2 = i0 + GRID_COLS as u16;
            let i3 = i2 + 1;
            indices.push(i0);
            indices.push(i2);
            indices.push(i1);
            indices.push(i1);
            indices.push(i2);
            indices.push(i3);
        }
    }
    indices
}
