//! Image-based lighting resources (ADR-0011 §8): an analytic sky baked into an equirect
//! texture, a diffuse irradiance equirect, six GGX-prefiltered equirects (one per roughness
//! level, no mips or cubemaps) and the split-sum BRDF LUT. All bakes are fullscreen fragment
//! passes recorded into the frame's command buffer, and only re-run when the [`Environment`]
//! changes.

use render_graph::{GraphBuilder, RenderGraph, TexId};
use rhi::{
    AddressMode, CompareOp, CullMode, DepthState, Device, Filter, Format, PipelineHandle, RhiError,
    SamplerDesc, SamplerHandle, TextureDesc, TextureHandle, TextureState, TextureUsage,
};

use crate::Environment;
use crate::passes::{BakeInputs, bake_pass};
use crate::renderer::{HDR_FORMAT, TONEMAP_VERT, make_pipeline};

static SKY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.frag.spv"));
static IRRADIANCE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/irradiance.frag.spv"));
static PREFILTER_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prefilter.frag.spv"));
static LUT_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/brdf_lut.frag.spv"));
static SKYBOX_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/skybox.vert.spv"));
static SKYBOX_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/skybox.frag.spv"));

/// Number of prefiltered roughness levels (roughness `i / 5`).
pub(crate) const PREFILTER_LEVELS: usize = 6;
const SKY_SIZE: (u32, u32) = (512, 256);
const IRRADIANCE_SIZE: (u32, u32) = (64, 32);
const PREFILTER_SIZES: [(u32, u32); PREFILTER_LEVELS] = [
    (256, 128),
    (128, 64),
    (64, 32),
    (64, 32),
    (32, 16),
    (32, 16),
];
const LUT_SIZE: (u32, u32) = (256, 256);

const PREFILTER_NAMES: [&str; PREFILTER_LEVELS] = [
    "ibl.prefilter0",
    "ibl.prefilter1",
    "ibl.prefilter2",
    "ibl.prefilter3",
    "ibl.prefilter4",
    "ibl.prefilter5",
];

/// The IBL textures imported into one frame's graph.
pub(crate) struct IblIds {
    pub sky: TexId,
    pub irradiance: TexId,
    pub lut: TexId,
    pub prefilter: [TexId; PREFILTER_LEVELS],
}

struct EnvTex {
    handle: TextureHandle,
    index: u32,
    extent: (u32, u32),
}

fn env_tex<D: Device>(
    device: &D,
    label: &str,
    extent: (u32, u32),
    format: Format,
) -> Result<EnvTex, RhiError> {
    let handle = device.create_texture(&TextureDesc {
        label: Some(label),
        width: extent.0,
        height: extent.1,
        format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    })?;
    Ok(EnvTex {
        handle,
        index: device.texture_index(handle)?,
        extent,
    })
}

/// Bindless slots handed to the shaders through the frame data.
pub(crate) struct IblIndices {
    pub sky: u32,
    pub irradiance: u32,
    pub lut: u32,
    pub sampler: u32,
    pub prefilter: [u32; PREFILTER_LEVELS],
}

pub(crate) struct Ibl {
    sky: EnvTex,
    irradiance: EnvTex,
    prefilter: Vec<EnvTex>,
    lut: EnvTex,
    sampler: SamplerHandle,
    sampler_index: u32,
    sky_pipeline: PipelineHandle,
    irradiance_pipeline: PipelineHandle,
    prefilter_pipeline: PipelineHandle,
    lut_pipeline: PipelineHandle,
    pub skybox: PipelineHandle,
    /// Environment of the last bake; `None` until the first bake.
    baked: Option<Environment>,
}

fn vec4(v: foundation::math::Vec3) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (c, f) in out.chunks_mut(4).zip([v.x, v.y, v.z, 0.0]) {
        c.copy_from_slice(&f.to_le_bytes());
    }
    out
}

impl Ibl {
    pub fn new<D: Device>(device: &D, graph: &mut RenderGraph) -> Result<Self, RhiError> {
        let pipe = |label, frag, format| {
            make_pipeline(
                device,
                label,
                TONEMAP_VERT,
                frag,
                &[format],
                None,
                CullMode::None,
            )
        };
        let sky_pipeline = pipe("ibl.sky", SKY_FRAG, HDR_FORMAT)?;
        let irradiance_pipeline = pipe("ibl.irradiance", IRRADIANCE_FRAG, HDR_FORMAT)?;
        let prefilter_pipeline = pipe("ibl.prefilter", PREFILTER_FRAG, HDR_FORMAT)?;
        let lut_pipeline = pipe("ibl.brdf_lut", LUT_FRAG, Format::Rg16Float)?;
        // Reverse-Z far plane: passes only where no geometry wrote depth; never writes depth.
        let skybox = make_pipeline(
            device,
            "ibl.skybox",
            SKYBOX_VERT,
            SKYBOX_FRAG,
            &[HDR_FORMAT],
            Some(DepthState {
                format: Format::D32Sfloat,
                compare: CompareOp::GreaterOrEqual,
                write: false,
                bias: None,
            }),
            CullMode::None,
        )?;
        let sky = env_tex(device, "ibl.sky", SKY_SIZE, HDR_FORMAT)?;
        let irradiance = env_tex(device, "ibl.irradiance", IRRADIANCE_SIZE, HDR_FORMAT)?;
        let mut prefilter = Vec::new();
        for (i, size) in PREFILTER_SIZES.iter().enumerate() {
            prefilter.push(env_tex(
                device,
                &format!("ibl.prefilter{i}"),
                *size,
                HDR_FORMAT,
            )?);
        }
        let lut = env_tex(device, "ibl.brdf_lut", LUT_SIZE, Format::Rg16Float)?;
        let sampler = device.create_sampler(&SamplerDesc {
            label: Some("ibl.equirect"),
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            address_u: AddressMode::Repeat, // longitude wraps
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::ClampToEdge,
            compare: None,
        })?;
        for t in [&sky, &irradiance, &lut].into_iter().chain(&prefilter) {
            graph.track(t.handle, TextureState::Undefined);
        }
        Ok(Self {
            sampler_index: device.sampler_index(sampler)?,
            sampler,
            sky,
            irradiance,
            prefilter,
            lut,
            sky_pipeline,
            irradiance_pipeline,
            prefilter_pipeline,
            lut_pipeline,
            skybox,
            baked: None,
        })
    }

    pub fn indices(&self) -> IblIndices {
        let mut prefilter = [0; PREFILTER_LEVELS];
        for (p, t) in prefilter.iter_mut().zip(&self.prefilter) {
            *p = t.index;
        }
        IblIndices {
            sky: self.sky.index,
            irradiance: self.irradiance.index,
            lut: self.lut.index,
            sampler: self.sampler_index,
            prefilter,
        }
    }

    /// Imports every IBL texture into the frame's graph (their states persist in the graph).
    pub fn import<D: Device>(&self, b: &mut GraphBuilder<'_, D>) -> IblIds {
        let mut imp = |name, t: &EnvTex| b.import(name, t.handle, t.extent, None);
        IblIds {
            sky: imp("ibl.sky", &self.sky),
            irradiance: imp("ibl.irradiance", &self.irradiance),
            lut: imp("ibl.brdf_lut", &self.lut),
            prefilter: std::array::from_fn(|i| imp(PREFILTER_NAMES[i], &self.prefilter[i])),
        }
    }

    /// True when `env` differs from the last baked one (or nothing was baked yet).
    pub fn needs_bake(&self, env: &Environment) -> bool {
        self.baked != Some(*env)
    }

    /// Adds the bake passes for `env` (everything, including the BRDF LUT, on the first call).
    /// Call [`Ibl::mark_baked`] once the graph has been recorded.
    // shortcut: the BRDF LUT is baked on the first render, not in Renderer::new, because the RHI
    // can only record into a frame's encoder; move it if a one-shot upload encoder appears.
    pub fn add_bakes<D: Device>(
        &self,
        b: &mut GraphBuilder<'_, D>,
        ids: &IblIds,
        env: &Environment,
    ) {
        let one = |b: &mut GraphBuilder<'_, D>, name, pipeline, target, read, push: &[u8]| {
            bake_pass(
                b,
                BakeInputs {
                    name,
                    pipeline,
                    target,
                    read,
                    push: push.to_vec(),
                },
            );
        };
        if self.baked.is_none() {
            one(b, "bake.brdf_lut", self.lut_pipeline, ids.lut, None, &[]);
        }
        let mut push = Vec::with_capacity(80);
        for v in [
            env.sun_direction,
            env.zenith,
            env.horizon,
            env.ground,
            env.sun_color,
        ] {
            push.extend_from_slice(&vec4(v));
        }
        one(b, "bake.sky", self.sky_pipeline, ids.sky, None, &push);
        let mut src = [0u8; 16];
        src[0..4].copy_from_slice(&self.sky.index.to_le_bytes());
        src[4..8].copy_from_slice(&self.sampler_index.to_le_bytes());
        let name = "bake.irradiance";
        one(
            b,
            name,
            self.irradiance_pipeline,
            ids.irradiance,
            Some(ids.sky),
            &src[..8],
        );
        for (i, &target) in ids.prefilter.iter().enumerate() {
            let roughness = i as f32 / (PREFILTER_LEVELS - 1) as f32;
            src[8..12].copy_from_slice(&roughness.to_le_bytes());
            one(
                b,
                "bake.prefilter",
                self.prefilter_pipeline,
                target,
                Some(ids.sky),
                &src,
            );
        }
    }

    /// Records that the bakes added for `env` were executed.
    pub fn mark_baked(&mut self, env: &Environment) {
        if self.baked.is_none() {
            tracing::info!("environment baked");
        } else {
            // Re-bakes happen every frame while the sun animates; keep them out of info.
            tracing::debug!("environment re-baked");
        }
        self.baked = Some(*env);
    }

    pub fn destroy<D: Device>(self, device: &D) {
        for t in [self.sky, self.irradiance, self.lut]
            .into_iter()
            .chain(self.prefilter)
        {
            device.destroy_texture(t.handle);
        }
        device.destroy_sampler(self.sampler);
        for p in [
            self.sky_pipeline,
            self.irradiance_pipeline,
            self.prefilter_pipeline,
            self.lut_pipeline,
            self.skybox,
        ] {
            device.destroy_pipeline(p);
        }
    }
}
