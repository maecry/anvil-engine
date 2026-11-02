//! Image-based lighting resources (ADR-0011 §8): an analytic sky baked into an equirect
//! texture, a diffuse irradiance equirect, six GGX-prefiltered equirects (one per roughness
//! level, no mips or cubemaps) and the split-sum BRDF LUT. All bakes are fullscreen fragment
//! passes recorded into the frame's command buffer, and only re-run when the [`Environment`]
//! changes.

use std::time::Instant;

use rhi::{
    AddressMode, CommandEncoder, CompareOp, CullMode, DepthState, Device, Filter, Format,
    PipelineHandle, RhiError, SamplerDesc, SamplerHandle, TextureDesc, TextureHandle, TextureUsage,
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
    baked: Option<Environment>,
    /// All textures are `ShaderRead` after the first bake.
    initialized: bool,
}

fn vec4(v: foundation::math::Vec3) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (c, f) in out.chunks_mut(4).zip([v.x, v.y, v.z, 0.0]) {
        c.copy_from_slice(&f.to_le_bytes());
    }
    out
}

impl Ibl {
    pub fn new<D: Device>(device: &D) -> Result<Self, RhiError> {
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
            initialized: false,
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

    /// Records the bakes if `env` differs from the last baked one (everything, including the
    /// BRDF LUT, on the first call). Each bake leaves its texture in `ShaderRead`.
    // shortcut: the BRDF LUT is baked on the first render, not in Renderer::new, because the RHI
    // can only record into a frame's encoder; move it if a one-shot upload encoder appears.
    pub fn bake<C: CommandEncoder>(&mut self, cmd: &mut C, env: &Environment) {
        if self.initialized && self.baked == Some(*env) {
            return;
        }
        let t0 = Instant::now();
        let init = self.initialized;
        let one = |cmd: &mut C, pipeline, t: &EnvTex, push: &[u8]| {
            bake_pass(
                cmd,
                &BakeInputs {
                    pipeline,
                    target: t.handle,
                    extent: t.extent,
                    initialized: init,
                    push,
                },
            );
        };
        if !init {
            one(cmd, self.lut_pipeline, &self.lut, &[]);
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
        one(cmd, self.sky_pipeline, &self.sky, &push);
        let mut src = [0u8; 16];
        src[0..4].copy_from_slice(&self.sky.index.to_le_bytes());
        src[4..8].copy_from_slice(&self.sampler_index.to_le_bytes());
        one(cmd, self.irradiance_pipeline, &self.irradiance, &src[..8]);
        for (i, t) in self.prefilter.iter().enumerate() {
            let roughness = i as f32 / (PREFILTER_LEVELS - 1) as f32;
            src[8..12].copy_from_slice(&roughness.to_le_bytes());
            one(cmd, self.prefilter_pipeline, t, &src);
        }
        self.initialized = true;
        self.baked = Some(*env);
        let record_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Re-bakes happen every frame while the sun animates; keep them out of info.
        if init {
            tracing::debug!(record_ms, "environment re-baked (command recording time)");
        } else {
            tracing::info!(record_ms, "environment baked (command recording time)");
        }
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
