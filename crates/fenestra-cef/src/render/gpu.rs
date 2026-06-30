use std::{collections::HashMap, sync::Arc};

use crate::style::Color;
use glyphon::cosmic_text::Align;
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};
use image::GenericImageView;
use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::render::rect_pipeline::{
    Globals, ImageVertex, RectVertex, create_image_pipeline, create_rounded_rect_pipeline,
    push_image_quad, push_rect_command, push_rounded_rect_command, to_wgpu_color,
};
use crate::render::{
    BorderCommand, DisplayCommand, DisplayList, RectCommand, RoundedRectCommand, ShadowCommand,
    TextCommand,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("failed to create GPU surface: {0}")]
    Surface(String),
    #[error("failed to request GPU adapter: {0}")]
    Adapter(String),
    #[error("failed to request GPU device: {0}")]
    Device(String),
    #[error("text renderer failed: {0}")]
    Text(String),
    #[error("surface validation failed")]
    SurfaceValidation,
    #[error("texture update failed: {0}")]
    Texture(String),
}

pub struct GpuRenderer {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    image_sampler: wgpu::Sampler,
    image_bind_group_layout: wgpu::BindGroupLayout,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffers: Vec<TextBufferEntry>,
    texture_cache: HashMap<String, CachedTexture>,
    scale_factor: f32,
    window: Arc<dyn Window>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct CachedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

impl GpuRenderer {
    pub async fn new(window: Arc<dyn Window>) -> Result<Self, RendererError> {
        let size = window.surface_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| RendererError::Surface(error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| RendererError::Adapter(error.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|error| RendererError::Device(error.to_string()))?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Fifo)
            .unwrap_or(capabilities.present_modes[0]);
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::PreMultiplied)
            .or_else(|| {
                capabilities
                    .alpha_modes
                    .iter()
                    .copied()
                    .find(|mode| *mode == wgpu::CompositeAlphaMode::PostMultiplied)
            })
            .unwrap_or(capabilities.alpha_modes[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &surface_config);

        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("stuk globals"),
            contents: bytemuck::cast_slice(&[Globals {
                viewport: [surface_config.width as f32, surface_config.height as f32],
                _padding: [0.0, 0.0],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("stuk globals bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stuk globals bind group"),
            layout: &globals_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });
        let pipeline = create_rounded_rect_pipeline(&device, format, &globals_bind_group_layout);
        let image_pipeline = create_image_pipeline(&device, format, &globals_bind_group_layout);
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stuk image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let image_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("stuk image per-texture bind group layout"),
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

        let font_system = FontSystem::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            instance,
            device,
            queue,
            surface,
            surface_config,
            pipeline,
            image_pipeline,
            image_sampler,
            image_bind_group_layout,
            globals_buffer,
            globals_bind_group,
            font_system,
            swash_cache: SwashCache::new(),
            viewport,
            atlas,
            text_renderer,
            text_buffers: Vec::new(),
            texture_cache: HashMap::new(),
            scale_factor: window.scale_factor() as f32,
            window,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f32) {
        if width == 0 || height == 0 {
            return;
        }

        self.surface_config.width = width;
        self.surface_config.height = height;
        self.scale_factor = scale_factor.max(0.25);
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn logical_size(&self) -> Size {
        Size::new(
            self.surface_config.width as f32 / self.scale_factor,
            self.surface_config.height as f32 / self.scale_factor,
        )
    }

    pub fn set_dynamic_bgra_image(
        &mut self,
        id: impl Into<String>,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Result<(), RendererError> {
        let id = id.into();
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "dynamic image has empty size".to_string(),
            ));
        }
        let expected_len = width as usize * height as usize * 4;
        if bytes.len() != expected_len {
            return Err(RendererError::Texture(format!(
                "dynamic image expected {expected_len} bytes, got {}",
                bytes.len()
            )));
        }

        let recreate = self
            .texture_cache
            .get(&id)
            .is_none_or(|entry| entry.width != width || entry.height != height);
        if recreate {
            self.create_dynamic_bgra_image(id.clone(), width, height);
        }

        let Some(entry) = self.texture_cache.get(&id) else {
            return Err(RendererError::Texture(
                "dynamic image was not cached".to_string(),
            ));
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub fn update_dynamic_bgra_image_region(
        &mut self,
        id: impl Into<String>,
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        region_width: u32,
        region_height: u32,
        bytes: &[u8],
    ) -> Result<(), RendererError> {
        let id = id.into();
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "dynamic image has empty size".to_string(),
            ));
        }
        if region_width == 0 || region_height == 0 {
            return Ok(());
        }
        if x >= width
            || y >= height
            || x.saturating_add(region_width) > width
            || y.saturating_add(region_height) > height
        {
            return Err(RendererError::Texture(format!(
                "dynamic image region {x},{y} {region_width}x{region_height} exceeds {width}x{height}"
            )));
        }
        let expected_len = width as usize * height as usize * 4;
        if bytes.len() != expected_len {
            return Err(RendererError::Texture(format!(
                "dynamic image expected {expected_len} bytes, got {}",
                bytes.len()
            )));
        }

        let recreate = self
            .texture_cache
            .get(&id)
            .is_none_or(|entry| entry.width != width || entry.height != height);
        if recreate {
            self.create_dynamic_bgra_image(id.clone(), width, height);
            return self.set_dynamic_bgra_image(id, width, height, bytes);
        }

        let Some(entry) = self.texture_cache.get(&id) else {
            return Err(RendererError::Texture(
                "dynamic image was not cached".to_string(),
            ));
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: (u64::from(y) * u64::from(width) + u64::from(x)) * 4,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width: region_width,
                height: region_height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub fn render(&mut self, display_list: &DisplayList) -> Result<(), RendererError> {
        self.queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::cast_slice(&[Globals {
                viewport: [
                    self.surface_config.width as f32,
                    self.surface_config.height as f32,
                ],
                _padding: [0.0, 0.0],
            }]),
        );
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width,
                height: self.surface_config.height,
            },
        );

        let rect_vertices = self.rect_vertices(display_list);
        let image_draws = self.image_draws(display_list);
        let clip_rect = find_clip_rect(display_list, self.scale_factor);
        self.rebuild_text_buffers(display_list);
        let text_areas = text_areas(&self.text_buffers, self.scale_factor);
        if !text_areas.is_empty() {
            self.text_renderer
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.font_system,
                    &mut self.atlas,
                    &self.viewport,
                    text_areas,
                    &mut self.swash_cache,
                )
                .map_err(|error| RendererError::Text(error.to_string()))?;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .map_err(|error| RendererError::Surface(error.to_string()))?;
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RendererError::SurfaceValidation);
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("stuk render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stuk render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(to_wgpu_color(display_list.background)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let Some(clip) = clip_rect {
                pass.set_scissor_rect(
                    clip.x as u32,
                    clip.y as u32,
                    clip.width as u32,
                    clip.height as u32,
                );
            }

            if !rect_vertices.is_empty() {
                let vertex_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("stuk rounded rect vertices"),
                            contents: bytemuck::cast_slice(&rect_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..rect_vertices.len() as u32, 0..1);
            }

            for draw in &image_draws {
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, &draw.bind_group, &[]);

                let vertex_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("stuk image vertices"),
                            contents: bytemuck::cast_slice(&draw.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..draw.vertices.len() as u32, 0..1);
            }

            if !self.text_buffers.is_empty() {
                self.text_renderer
                    .render(&self.atlas, &self.viewport, &mut pass)
                    .map_err(|error| RendererError::Text(error.to_string()))?;
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();
        Ok(())
    }

    fn image_draws(&mut self, display_list: &DisplayList) -> Vec<ImageDraw> {
        let mut draws = Vec::new();
        for command in &display_list.commands {
            let DisplayCommand::Image(image) = command else {
                continue;
            };
            let path = image.id.clone();
            let cached = self.texture_cache.get(&path).cloned();
            let bind_group = match cached {
                Some(entry) => entry.bind_group,
                None => match self.load_image_texture(&path) {
                    Ok(bind_group) => bind_group,
                    Err(_) => continue,
                },
            };
            let mut vertices = Vec::new();
            push_image_quad(
                &mut vertices,
                image.x,
                image.y,
                image.width,
                image.height,
                self.scale_factor,
            );
            draws.push(ImageDraw {
                bind_group,
                vertices,
            });
        }
        draws
    }

    fn load_image_texture(&mut self, path: &str) -> Result<wgpu::BindGroup, String> {
        let img = image::open(path).map_err(|e| format!("failed to load image {path}: {e}"))?;
        let dimensions = img.dimensions();
        let rgba = img.to_rgba8();
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(path),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * dimensions.0),
                rows_per_image: Some(dimensions.1),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(path),
            layout: &self.image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.texture_cache.insert(
            path.to_string(),
            CachedTexture {
                texture,
                view,
                bind_group: bind_group.clone(),
                width: dimensions.0,
                height: dimensions.1,
            },
        );
        Ok(bind_group)
    }

    fn create_dynamic_bgra_image(&mut self, id: String, width: u32, height: u32) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&id),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&id),
            layout: &self.image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.texture_cache.insert(
            id,
            CachedTexture {
                texture,
                view,
                bind_group,
                width,
                height,
            },
        );
    }

    fn rect_vertices(&self, display_list: &DisplayList) -> Vec<RectVertex> {
        let mut vertices = Vec::new();
        for command in &display_list.commands {
            match command {
                DisplayCommand::Rect(command) => {
                    push_rect_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::RoundedRect(command) => {
                    push_rounded_rect_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::Border(command) => {
                    push_border_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::Material(command) => {
                    push_rounded_rect_command(
                        &mut vertices,
                        &RoundedRectCommand {
                            x: command.x,
                            y: command.y,
                            width: command.width,
                            height: command.height,
                            radius: command.radius,
                            color: command.fallback,
                        },
                        self.scale_factor,
                    );
                }
                DisplayCommand::Shadow(command) => {
                    push_shadow_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::Text(_)
                | DisplayCommand::Image(_)
                | DisplayCommand::Svg(_)
                | DisplayCommand::Clip(_)
                | DisplayCommand::Transform(_) => {}
            }
        }
        vertices
    }

    fn rebuild_text_buffers(&mut self, display_list: &DisplayList) {
        self.text_buffers.clear();
        for command in &display_list.commands {
            let DisplayCommand::Text(command) = command else {
                continue;
            };

            let scale = self.scale_factor;
            let mut buffer = Buffer::new(
                &mut self.font_system,
                Metrics::new(command.size * scale, command.line_height * scale),
            );
            buffer.set_size(
                &mut self.font_system,
                Some(command.width * scale),
                Some(command.height * scale),
            );
            buffer.set_wrap(&mut self.font_system, to_glyphon_wrap(command.wrap));
            buffer.set_text(
                &mut self.font_system,
                &command.text,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                Some(to_glyphon_align(command.align)),
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.text_buffers.push(TextBufferEntry {
                buffer,
                command: command.clone(),
            });
        }
    }
}

struct TextBufferEntry {
    buffer: Buffer,
    command: TextCommand,
}

fn text_areas(text_buffers: &[TextBufferEntry], scale: f32) -> Vec<TextArea<'_>> {
    text_buffers
        .iter()
        .map(|entry| TextArea {
            buffer: &entry.buffer,
            left: entry.command.x * scale,
            top: entry.command.y * scale,
            scale: 1.0,
            bounds: TextBounds {
                left: (entry.command.x * scale) as i32,
                top: (entry.command.y * scale) as i32,
                right: ((entry.command.x + entry.command.width) * scale) as i32,
                bottom: ((entry.command.y + entry.command.height) * scale) as i32,
            },
            default_color: to_glyphon_color(entry.command.color),
            custom_glyphs: &[],
        })
        .collect()
}

fn to_glyphon_color(color: Color) -> glyphon::Color {
    glyphon::Color::rgba(
        to_u8(color.r),
        to_u8(color.g),
        to_u8(color.b),
        to_u8(color.a),
    )
}

fn to_glyphon_align(align: crate::style::TextAlign) -> Align {
    match align {
        crate::style::TextAlign::Start => Align::Left,
        crate::style::TextAlign::Center => Align::Center,
        crate::style::TextAlign::End => Align::Right,
    }
}

fn to_glyphon_wrap(wrap: crate::style::TextWrap) -> Wrap {
    match wrap {
        crate::style::TextWrap::Normal => Wrap::None,
        crate::style::TextWrap::Pretty | crate::style::TextWrap::Balance => Wrap::WordOrGlyph,
    }
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn push_border_command(vertices: &mut Vec<RectVertex>, command: &BorderCommand, scale: f32) {
    let thickness = command.thickness.max(0.0);
    if thickness == 0.0 {
        return;
    }

    let top = RectCommand {
        x: command.x,
        y: command.y,
        width: command.width,
        height: thickness,
        color: command.color,
    };
    let bottom = RectCommand {
        x: command.x,
        y: command.y + command.height - thickness,
        width: command.width,
        height: thickness,
        color: command.color,
    };
    let left = RectCommand {
        x: command.x,
        y: command.y,
        width: thickness,
        height: command.height,
        color: command.color,
    };
    let right = RectCommand {
        x: command.x + command.width - thickness,
        y: command.y,
        width: thickness,
        height: command.height,
        color: command.color,
    };

    for rect in [top, bottom, left, right] {
        push_rect_command(vertices, &rect, scale);
    }
}

fn push_shadow_command(vertices: &mut Vec<RectVertex>, command: &ShadowCommand, scale: f32) {
    let spread = command.spread.max(0.0);
    push_rounded_rect_command(
        vertices,
        &RoundedRectCommand {
            x: command.x + command.offset_x - spread,
            y: command.y + command.offset_y - spread,
            width: command.width + spread * 2.0,
            height: command.height + spread * 2.0,
            radius: command.radius + spread,
            color: command.color,
        },
        scale,
    );
}

struct ImageDraw {
    bind_group: wgpu::BindGroup,
    vertices: Vec<ImageVertex>,
}

fn find_clip_rect(
    display_list: &DisplayList,
    scale: f32,
) -> Option<crate::render::display_list::Rect> {
    for command in &display_list.commands {
        if let DisplayCommand::Clip(clip) = command {
            let x = (clip.x * scale) as u32;
            let y = (clip.y * scale) as u32;
            let width = (clip.width * scale).ceil() as u32;
            let height = (clip.height * scale).ceil() as u32;
            return Some(crate::render::display_list::Rect::new(
                x as f32,
                y as f32,
                width as f32,
                height as f32,
            ));
        }
    }
    None
}
