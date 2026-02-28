use std::time::Instant;

use tracing::{debug, info};
use wgpu::util::DeviceExt;

use crate::backend::{ComputeBackend, ComputeBackendType, ComputeResult};

/// GPU compute backend using wgpu (Vulkan/Metal/DX12).
pub struct WgpuCompute {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl WgpuCompute {
    /// Attempt to create a new wgpu compute backend.
    /// Returns `None` if no suitable GPU adapter is available.
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;

        info!(
            "wgpu adapter: {:?} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("architect-compute"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()?;

        Some(Self { device, queue })
    }

    /// Run a compute shader with the given WGSL source, input buffers, output
    /// buffer size, and workgroup dispatch dimensions.
    fn run_shader(
        &self,
        shader_src: &str,
        inputs: &[&[u8]],
        output_size: u64,
        dispatch: (u32, u32, u32),
    ) -> Vec<u8> {
        let shader_module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("compute_shader"),
                source: wgpu::ShaderSource::Wgsl(shader_src.into()),
            });

        // Create input storage buffers.
        let input_buffers: Vec<wgpu::Buffer> = inputs
            .iter()
            .enumerate()
            .map(|(i, data)| {
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("input_buffer_{}", i)),
                        contents: data,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    })
            })
            .collect();

        // Create output storage buffer.
        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output_buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Create staging buffer for readback.
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging_buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Build bind group layout entries.
        let mut layout_entries: Vec<wgpu::BindGroupLayoutEntry> = input_buffers
            .iter()
            .enumerate()
            .map(|(i, _)| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();

        // Output buffer binding.
        layout_entries.push(wgpu::BindGroupLayoutEntry {
            binding: input_buffers.len() as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("compute_bind_group_layout"),
                    entries: &layout_entries,
                });

        // Build bind group entries.
        let mut bg_entries: Vec<wgpu::BindGroupEntry> = input_buffers
            .iter()
            .enumerate()
            .map(|(i, buf)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: buf.as_entire_binding(),
            })
            .collect();

        bg_entries.push(wgpu::BindGroupEntry {
            binding: input_buffers.len() as u32,
            resource: output_buffer.as_entire_binding(),
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compute_bind_group"),
            layout: &bind_group_layout,
            entries: &bg_entries,
        });

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("compute_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("compute_pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader_module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });

        // Encode and submit commands.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("compute_encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compute_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(dispatch.0, dispatch.1, dispatch.2);
        }

        encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map the staging buffer and read the results.
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("Failed to receive map result")
            .expect("Failed to map staging buffer");

        let data = buffer_slice.get_mapped_range().to_vec();
        staging_buffer.unmap();
        data
    }
}

impl ComputeBackend for WgpuCompute {
    fn matmul(&self, a: &[f32], b: &[f32], m: usize, n: usize, k: usize) -> ComputeResult {
        assert_eq!(a.len(), m * k, "Matrix A dimensions mismatch");
        assert_eq!(b.len(), k * n, "Matrix B dimensions mismatch");

        let start = Instant::now();

        // Uniform buffer: [M, N, K, _padding]
        let dims: [u32; 4] = [m as u32, n as u32, k as u32, 0];
        let dims_bytes = bytemuck::cast_slice(&dims);
        let a_bytes = bytemuck::cast_slice(a);
        let b_bytes = bytemuck::cast_slice(b);

        let output_size = (m * n * std::mem::size_of::<f32>()) as u64;

        let shader_src = r#"
struct Dims {
    M: u32,
    N: u32,
    K: u32,
    _pad: u32,
};

@group(0) @binding(0) var<storage, read> dims: Dims;
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> result: array<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    let col = gid.y;

    if (row >= dims.M || col >= dims.N) {
        return;
    }

    var sum: f32 = 0.0;
    for (var i: u32 = 0u; i < dims.K; i = i + 1u) {
        sum = sum + a[row * dims.K + i] * b[i * dims.N + col];
    }
    result[row * dims.N + col] = sum;
}
"#;

        let workgroups_x = (m as u32 + 15) / 16;
        let workgroups_y = (n as u32 + 15) / 16;

        let raw = self.run_shader(
            shader_src,
            &[dims_bytes, a_bytes, b_bytes],
            output_size,
            (workgroups_x, workgroups_y, 1),
        );

        let data: Vec<f32> = bytemuck::cast_slice(&raw).to_vec();

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!("wgpu matmul {}x{}x{} completed in {}ms", m, k, n, duration_ms);

        ComputeResult { data, duration_ms }
    }

    fn vector_add(&self, a: &[f32], b: &[f32]) -> ComputeResult {
        assert_eq!(a.len(), b.len(), "Vector dimensions mismatch");

        let start = Instant::now();

        let len = a.len();
        let len_u32: [u32; 1] = [len as u32];
        let len_bytes = bytemuck::cast_slice(&len_u32);
        let a_bytes = bytemuck::cast_slice(a);
        let b_bytes = bytemuck::cast_slice(b);

        let output_size = (len * std::mem::size_of::<f32>()) as u64;

        let shader_src = r#"
@group(0) @binding(0) var<storage, read> length: u32;
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> result: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= length) {
        return;
    }
    result[idx] = a[idx] + b[idx];
}
"#;

        let workgroups = (len as u32 + 255) / 256;

        let raw = self.run_shader(
            shader_src,
            &[len_bytes, a_bytes, b_bytes],
            output_size,
            (workgroups, 1, 1),
        );

        let data: Vec<f32> = bytemuck::cast_slice(&raw).to_vec();

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!("wgpu vector_add len={} completed in {}ms", len, duration_ms);

        ComputeResult { data, duration_ms }
    }

    fn dot_product(&self, a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len(), "Vector dimensions mismatch");

        // Element-wise multiply on GPU, then sum on CPU.
        let len = a.len();
        let len_u32: [u32; 1] = [len as u32];
        let len_bytes = bytemuck::cast_slice(&len_u32);
        let a_bytes = bytemuck::cast_slice(a);
        let b_bytes = bytemuck::cast_slice(b);

        let output_size = (len * std::mem::size_of::<f32>()) as u64;

        let shader_src = r#"
@group(0) @binding(0) var<storage, read> length: u32;
@group(0) @binding(1) var<storage, read> a: array<f32>;
@group(0) @binding(2) var<storage, read> b: array<f32>;
@group(0) @binding(3) var<storage, read_write> result: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= length) {
        return;
    }
    result[idx] = a[idx] * b[idx];
}
"#;

        let workgroups = (len as u32 + 255) / 256;

        let raw = self.run_shader(
            shader_src,
            &[len_bytes, a_bytes, b_bytes],
            output_size,
            (workgroups, 1, 1),
        );

        let products: &[f32] = bytemuck::cast_slice(&raw);
        products.iter().sum()
    }

    fn process_shard(&self, data: &[f32], scale: f32) -> ComputeResult {
        let start = Instant::now();

        let len = data.len();

        // Pack length and scale into a uniform struct: [len: u32, scale: f32]
        let params: [u32; 2] = [len as u32, scale.to_bits()];
        let params_bytes = bytemuck::cast_slice(&params);
        let data_bytes = bytemuck::cast_slice(data);

        let output_size = (len * std::mem::size_of::<f32>()) as u64;

        let shader_src = r#"
struct Params {
    length: u32,
    scale_bits: u32,
};

@group(0) @binding(0) var<storage, read> params: Params;
@group(0) @binding(1) var<storage, read> data: array<f32>;
@group(0) @binding(2) var<storage, read_write> result: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.length) {
        return;
    }
    let scale = bitcast<f32>(params.scale_bits);
    result[idx] = data[idx] * scale;
}
"#;

        let workgroups = (len as u32 + 255) / 256;

        let raw = self.run_shader(
            shader_src,
            &[params_bytes, data_bytes],
            output_size,
            (workgroups, 1, 1),
        );

        let result: Vec<f32> = bytemuck::cast_slice(&raw).to_vec();

        let duration_ms = start.elapsed().as_millis() as u64;
        debug!(
            "wgpu process_shard len={} scale={} completed in {}ms",
            len, scale, duration_ms
        );

        ComputeResult {
            data: result,
            duration_ms,
        }
    }

    fn backend_type(&self) -> ComputeBackendType {
        ComputeBackendType::Wgpu
    }
}
