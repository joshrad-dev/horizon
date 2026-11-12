//! iPhone Duo held upright: the game draws offscreen at the size of the screen above the fold,
//! and the deck presenter owns the window's surface. Each frame it puts the game's frame above
//! the fold and the control deck below it: a dark surface that reflects the game (the frame
//! mirrored across the fold, sharp at the fold and blurring and fading with distance, as a
//! glossy surface under a screen does; the technique of Mirage's Duo keyboard), with the deck's
//! own UI (a HUD batch) over it.
use super::Renderer;
use crate::Result;
use crate::hud::HudBatch;
use std::sync::Arc;

/// Where the deck sits on the window, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeckLayout {
    /// The window.
    pub full: [u32; 2],
    /// The game's frame: the top `game_height` rows.
    pub game_height: u32,
    /// The deck: from this row to the bottom (the fold is between).
    pub deck_top: u32,
}
impl DeckLayout {
    pub fn deck_size(&self) -> [u32; 2] {
        [
            self.full[0],
            self.full[1].saturating_sub(self.deck_top).max(1),
        ]
    }
}

/// The reflection probe: the game's frame, small, with a mip chain the deck blurs with.
const PROBE: [u32; 2] = [256, 128];
const PROBE_MIPS: u32 = 8;
const PROBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    /// Window width and height, game height, deck top.
    geometry: [f32; 4],
    /// Reflection strength, clarity, glow strength, the card's corner radius (pixels).
    look: [f32; 4],
    /// The recessed card: x, y, width, height (window pixels).
    card: [f32; 4],
}

pub struct DeckPresenter {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    layout: DeckLayout,
    composite: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    downsample_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    probe: wgpu::Texture,
    ui: wgpu::Texture,
    ui_depth: wgpu::TextureView,
    uniform: wgpu::Buffer,
}

const SHADER: &str = r#"
struct Uniform { geometry: vec4<f32>, look: vec4<f32>, card: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniform;
@group(0) @binding(1) var game: texture_2d<f32>;
@group(0) @binding(2) var probe: texture_2d<f32>;
@group(0) @binding(3) var ui: texture_2d<f32>;
@group(0) @binding(4) var linear: sampler;

struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

// Signed distance to a rounded rectangle (negative inside).
fn rounded(p: vec2<f32>, rect: vec4<f32>, r: f32) -> f32 {
    let c = rect.xy + rect.zw * 0.5;
    let q = abs(p - c) - rect.zw * 0.5 + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// The game mirrored across the fold, as brushed metal reflects it: nearly sharp at the fold,
// blurring more and more with distance, and smeared along the reflection (vertically) more
// than across it.
fn reflect_metal(uv: vec2<f32>, d: f32, clarity: f32) -> vec3<f32> {
    let spread = pow(d, 0.8);
    let lod = mix(0.6, 6.5, spread) + (1.0 - clarity) * 0.5;
    let reach = vec2<f32>(0.004 + spread * 0.02, 0.01 + spread * 0.16);
    var c = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = -4; i <= 4; i++) {
        let t = f32(i) / 4.0;
        let w = 1.0 - abs(t) * 0.6;
        let o = vec2<f32>(sin(t * 3.7) * reach.x, t * reach.y);
        c += textureSampleLevel(probe, linear, clamp(uv + o, vec2<f32>(0.0), vec2<f32>(1.0)), lod).rgb * w;
        total += w;
    }
    return c / total;
}

@fragment fn composite(in: Out) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    let width = u.geometry.x;
    let game_h = u.geometry.z;
    let deck_top = u.geometry.w;
    if (p.y < game_h) {
        return vec4<f32>(textureLoad(game, vec2<i32>(p), 0).rgb, 1.0);
    }
    let deck_h = max(1.0, u.geometry.y - deck_top);
    // 0 at the fold, 1 at the bottom of the deck.
    let d = clamp((p.y - deck_top) / deck_h, 0.0, 1.0);
    // Anodised graphite, faintly brushed across.
    let grain = fract(sin(floor(p.y * 0.5) * 12.9898) * 43758.547) * 0.012;
    var color = mix(vec3<f32>(0.075, 0.078, 0.086), vec3<f32>(0.030, 0.031, 0.036), d) + grain;
    let mirrored = clamp(1.0 - (p.y - deck_top) / max(1.0, game_h) * 1.1, 0.0, 1.0);
    let uv = vec2<f32>(p.x / width, mirrored);
    var reflection = reflect_metal(uv, d, u.look.y);
    // Metal reflects its own colour: mostly grey, a little of the scene's hue, cool.
    let luma = dot(reflection, vec3<f32>(0.2126, 0.7152, 0.0722));
    reflection = mix(vec3<f32>(luma), reflection, 0.35) * vec3<f32>(0.92, 0.95, 1.0);
    let fade = pow(1.0 - d, 1.6) * (1.0 - smoothstep(0.9, 1.0, d));
    // Brighter where the angle to the screen is shallow, at the fold.
    let fresnel = mix(1.0, 0.45, sqrt(d));
    var shine = reflection * u.look.x * fade * fresnel;
    // Screen light spilling onto the deck near the fold.
    let glow = textureSampleLevel(probe, linear, vec2<f32>(uv.x, 0.95), 5.0).rgb;
    shine += vec3<f32>(dot(glow, vec3<f32>(0.33))) * u.look.z * exp(-d * 9.0);
    if (p.y < deck_top) {
        // The fold itself: a dark seam.
        return vec4<f32>((color + shine) * 0.3, 1.0);
    }
    // The card, pressed into the deck: darker, a dimmer reflection, shadowed under its upper
    // edge and lit along its lower lip.
    let r = u.look.w;
    let sd = rounded(p, u.card, r);
    let inside = 1.0 - smoothstep(-1.0, 1.0, sd);
    let depth = clamp(-sd / 18.0, 0.0, 1.0);
    let top_edge = clamp((p.y - u.card.y) / 30.0, 0.0, 1.0);
    let card_color = color * 0.55 + shine * 0.3;
    let shadow = mix(0.45, 1.0, top_edge) * mix(0.8, 1.0, depth);
    var result = mix(color + shine, card_color * shadow, inside);
    // The lip: a hairline highlight just outside the card's lower half, a dark line above.
    let rim = 1.0 - smoothstep(0.0, 2.0, abs(sd - 1.0));
    let below = clamp((p.y - (u.card.y + u.card.w * 0.5)) / (u.card.w * 0.5), -1.0, 1.0);
    result += vec3<f32>(rim * max(below, 0.0) * 0.10);
    result *= 1.0 - rim * max(-below, 0.0) * 0.5;
    // The pages' content, inside the card only.
    let ui_px = vec2<i32>(vec2<f32>(p.x, p.y - deck_top));
    let ui_c = textureLoad(ui, ui_px, 0) * inside;
    return vec4<f32>(result * (1.0 - ui_c.a) + ui_c.rgb, 1.0);
}
"#;

const DOWNSAMPLE: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var down_sampler: sampler;

struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

// One mip from the level above: a 3x3 tent, wider than a box, so the blur stays smooth.
@fragment fn down(in: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    var c = vec3<f32>(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let w = (2.0 - abs(f32(x))) * (2.0 - abs(f32(y)));
            c += textureSampleLevel(source, down_sampler, in.uv + vec2<f32>(f32(x), f32(y)) / size, 0.0).rgb * w;
        }
    }
    return vec4<f32>(c / 16.0, 1.0);
}
"#;

impl DeckPresenter {
    fn new(
        device: &wgpu::Device,
        surface: wgpu::Surface<'static>,
        format: wgpu::TextureFormat,
        ui_format: wgpu::TextureFormat,
        layout: DeckLayout,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Deck"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let down_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Deck downsample"),
            source: wgpu::ShaderSource::Wgsl(DOWNSAMPLE.into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Deck composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1),
                texture(2),
                texture(3),
                sampler_entry(4),
            ],
        });
        let downsample_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Deck downsample"),
            entries: &[texture(0), sampler_entry(1)],
        });
        let pipeline =
            |module: &wgpu::ShaderModule, layout: &wgpu::BindGroupLayout, entry: &str, format| {
                let pipeline_layout =
                    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("Deck"),
                        bind_group_layouts: &[Some(layout)],
                        immediate_size: 0,
                    });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Deck"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(entry),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let composite = pipeline(&module, &composite_layout, "composite", format);
        let downsample = pipeline(&down_module, &downsample_layout, "down", PROBE_FORMAT);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Deck"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let probe = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Deck reflection probe"),
            size: wgpu::Extent3d {
                width: PROBE[0],
                height: PROBE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: PROBE_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PROBE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Deck"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: layout.full[0].max(1),
            height: layout.full[1].max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::default(),
        };
        surface.configure(device, &config);
        let (ui, ui_depth) = Self::ui_targets(device, ui_format, layout.deck_size());
        DeckPresenter {
            surface,
            config,
            layout,
            composite,
            downsample,
            composite_layout,
            downsample_layout,
            sampler,
            probe,
            ui,
            ui_depth,
            uniform,
        }
    }

    fn ui_targets(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        [width, height]: [u32; 2],
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let ui = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Deck UI"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Deck UI depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        (ui, depth)
    }

    fn relayout(&mut self, device: &wgpu::Device, layout: DeckLayout) {
        if layout == self.layout {
            return;
        }
        if layout.full != self.layout.full {
            self.config.width = layout.full[0].max(1);
            self.config.height = layout.full[1].max(1);
            self.surface.configure(device, &self.config);
        }
        if layout.deck_size() != self.layout.deck_size() {
            (self.ui, self.ui_depth) =
                Self::ui_targets(device, self.ui.format(), layout.deck_size());
        }
        self.layout = layout;
    }

    fn pass<'a>(
        encoder: &'a mut wgpu::CommandEncoder,
        target: &'a wgpu::TextureView,
        clear: Option<wgpu::Color>,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Deck"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        })
    }
}

impl Renderer {
    /// Whether the deck presents the window (iPhone Duo upright).
    pub fn deck(&self) -> Option<DeckLayout> {
        self.deck.as_ref().map(|d| d.layout)
    }

    /// Lays the window out with the deck below the fold, or without one. With the deck the
    /// renderer draws offscreen at the size of the game's part and the deck presents; without,
    /// the renderer takes the window's surface back.
    pub fn set_deck(&mut self, layout: Option<DeckLayout>) -> Result<()> {
        match (layout, self.deck.is_some()) {
            (Some(layout), true) => {
                if let Some(deck) = &mut self.deck {
                    deck.relayout(&self.device, layout);
                }
                if [self.config.width, self.config.height] != [layout.full[0], layout.game_height] {
                    self.resize(layout.full[0], layout.game_height.max(1));
                }
            }
            (Some(layout), false) => {
                self.frame_gen.stop();
                self.queue.wait();
                let Some(surface) = self.surface.take() else {
                    return Err("the deck needs the window's surface".into());
                };
                let surface = match Arc::try_unwrap(surface) {
                    Ok(surface) => surface,
                    Err(shared) => {
                        self.surface = Some(shared);
                        return Err("the window's surface is still shared".into());
                    }
                };
                let format = self.config.format;
                self.deck = Some(DeckPresenter::new(
                    &self.device,
                    surface,
                    format,
                    self.output.frame.format,
                    layout,
                ));
                self.resize(layout.full[0], layout.game_height.max(1));
                log::info!(
                    "Horizon renderer: deck below the fold ({}x{} game, deck from row {})",
                    layout.full[0],
                    layout.game_height,
                    layout.deck_top
                );
            }
            (None, true) => {
                self.queue.wait();
                let deck = self.deck.take().expect("checked");
                let full = deck.layout.full;
                self.surface = Some(Arc::new(deck.surface));
                self.offscreen = None;
                self.resize(full[0], full[1]);
                log::info!("Horizon renderer: deck off");
            }
            (None, false) => {}
        }
        Ok(())
    }

    /// The window's frame with the deck: the game's last frame above the fold, the deck with
    /// its reflection, the recessed `card` (window pixels, corners of radius `corner`) and `ui`
    /// (a batch the size of the deck, shown in the card) below. Call after `render`.
    pub fn present_deck(
        &mut self,
        ui: &HudBatch,
        look: [f32; 3],
        card: [f32; 4],
        corner: f32,
    ) -> Result<bool> {
        let Some(game) = self.offscreen.clone() else {
            return Ok(false);
        };
        let Some(deck) = &mut self.deck else {
            return Ok(false);
        };
        let frame = match deck.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                deck.surface.configure(&self.device, &deck.config);
                return Ok(false);
            }
            _ => return Ok(false),
        };
        let layout = deck.layout;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Deck"),
            });
        // The deck's UI, into its own target (any frame format), clear for blending.
        let ui_view = deck.ui.create_view(&Default::default());
        drop(DeckPresenter::pass(
            &mut encoder,
            &ui_view,
            Some(wgpu::Color::TRANSPARENT),
        ));
        let size = layout.deck_size();
        self.hud
            .prepare(&self.device, &mut self.uploads, &mut encoder, ui, size);
        self.hud
            .draw(&mut encoder, &ui_view, &deck.ui_depth, ui, size);
        // The probe: the game's frame, then each mip from the one above.
        let game_view = game.create_view(&Default::default());
        let mip = |level| {
            deck.probe.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for level in 0..PROBE_MIPS {
            let source = if level == 0 {
                game_view.clone()
            } else {
                mip(level - 1)
            };
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Deck downsample"),
                layout: &deck.downsample_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&deck.sampler),
                    },
                ],
            });
            let target = mip(level);
            let mut pass = DeckPresenter::pass(&mut encoder, &target, None);
            pass.set_pipeline(&deck.downsample);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.write_buffer(
            &deck.uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                geometry: [
                    layout.full[0] as f32,
                    layout.full[1] as f32,
                    layout.game_height as f32,
                    layout.deck_top as f32,
                ],
                look: [look[0], look[1], look[2], corner],
                card,
            }),
        );
        let probe_view = deck.probe.create_view(&Default::default());
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Deck composite"),
            layout: &deck.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: deck.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&game_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&probe_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ui_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&deck.sampler),
                },
            ],
        });
        let target = frame.texture.create_view(&Default::default());
        {
            let mut pass = DeckPresenter::pass(&mut encoder, &target, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&deck.composite);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.uploads.finish(&encoder);
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        for source in [SHADER, DOWNSAMPLE] {
            let module = naga::front::wgsl::parse_str(source).expect("parses");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("validates");
        }
    }
}
