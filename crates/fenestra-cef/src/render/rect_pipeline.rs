use crate::style::Color;
use bytemuck::{Pod, Zeroable};

use crate::render::{RectCommand, RoundedRectCommand};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Globals {
    pub viewport: [f32; 2],
    pub _padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct RectVertex {
    position: [f32; 2],
    rect_origin: [f32; 2],
    rect_size: [f32; 2],
    color: [f32; 4],
    radius: f32,
}

pub(crate) fn create_rounded_rect_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x2,
        3 => Float32x4,
        4 => Float32
    ];

    let shader = device.create_shader_module(wgpu::include_wgsl!("rounded_rect.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("stuk rounded rect pipeline layout"),
        bind_group_layouts: &[Some(globals_bind_group_layout)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("stuk rounded rect pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<RectVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(crate) fn push_rect_command(vertices: &mut Vec<RectVertex>, command: &RectCommand, scale: f32) {
    push_rect(
        vertices,
        command.x,
        command.y,
        command.width,
        command.height,
        0.0,
        command.color,
        scale,
    );
}

pub(crate) fn push_rounded_rect_command(
    vertices: &mut Vec<RectVertex>,
    command: &RoundedRectCommand,
    scale: f32,
) {
    push_rect(
        vertices,
        command.x,
        command.y,
        command.width,
        command.height,
        command.radius,
        command.color,
        scale,
    );
}

pub(crate) fn to_wgpu_color(color: Color) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(color.r),
        g: f64::from(color.g),
        b: f64::from(color.b),
        a: f64::from(color.a),
    }
}

fn push_rect(
    vertices: &mut Vec<RectVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    color: Color,
    scale: f32,
) {
    let x = x * scale;
    let y = y * scale;
    let width = width * scale;
    let height = height * scale;
    let radius = radius * scale;
    let rect_origin = [x, y];
    let rect_size = [width, height];
    let color = [color.r, color.g, color.b, color.a];
    let points = [
        [x, y],
        [x + width, y],
        [x + width, y + height],
        [x, y],
        [x + width, y + height],
        [x, y + height],
    ];

    vertices.extend(points.map(|position| RectVertex {
        position,
        rect_origin,
        rect_size,
        color,
        radius,
    }));
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ImageVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

pub(crate) fn create_image_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

    let shader = device.create_shader_module(wgpu::include_wgsl!("image.wgsl"));

    let sampler_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stuk image bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("stuk image pipeline layout"),
        bind_group_layouts: &[
            Some(globals_bind_group_layout),
            Some(&sampler_bind_group_layout),
        ],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("stuk image pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ImageVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(crate) fn push_image_quad(
    vertices: &mut Vec<ImageVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
) {
    let x = x * scale;
    let y = y * scale;
    let width = width * scale;
    let height = height * scale;

    let positions = [
        [x, y],
        [x + width, y],
        [x + width, y + height],
        [x, y],
        [x + width, y + height],
        [x, y + height],
    ];
    let uvs = [
        [0.0f32, 0.0f32],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];

    for i in 0..6 {
        vertices.push(ImageVertex {
            position: positions[i],
            uv: uvs[i],
        });
    }
}
