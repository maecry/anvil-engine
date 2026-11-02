//! Compiles the GLSL shaders to SPIR-V in `OUT_DIR` (no committed .spv files). The Vulkan 1.3
//! target environment is required for `GL_EXT_buffer_reference` and `nonuniformEXT`.

use std::{env, fs, path::PathBuf};

fn make_options<'a>() -> shaderc::CompileOptions<'a> {
    let mut options = shaderc::CompileOptions::new().expect("shaderc options");
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_3 as u32,
    );
    options.set_target_spirv(shaderc::SpirvVersion::V1_6);
    options.set_include_callback(|name, _, _, _| {
        let path = format!("shaders/{name}");
        let content = fs::read_to_string(&path).map_err(|e| format!("include {path}: {e}"))?;
        Ok(shaderc::ResolvedInclude {
            resolved_name: path,
            content,
        })
    });
    options
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let compiler = shaderc::Compiler::new().expect("shaderc compiler");
    use shaderc::ShaderKind::{Fragment, Vertex};
    // (source file, output stem, kind, defines): `MASK` variants add the alpha test.
    let mask = Some("MASK");
    for (file, out_name, kind, define) in [
        ("forward.vert", "forward.vert", Vertex, None),
        ("forward.frag", "forward.frag", Fragment, None),
        ("forward.frag", "forward_mask.frag", Fragment, mask),
        ("prepass.frag", "prepass.frag", Fragment, None),
        ("prepass.frag", "prepass_mask.frag", Fragment, mask),
        ("gbuffer.frag", "gbuffer.frag", Fragment, None),
        ("gbuffer.frag", "gbuffer_mask.frag", Fragment, mask),
        ("lighting.frag", "lighting.frag", Fragment, None),
        ("ssao.frag", "ssao.frag", Fragment, None),
        ("ssao_blur.frag", "ssao_blur.frag", Fragment, None),
        ("taa.frag", "taa.frag", Fragment, None),
        ("bloom_down.frag", "bloom_down.frag", Fragment, None),
        ("bloom_up.frag", "bloom_up.frag", Fragment, None),
        ("shadow.vert", "shadow.vert", Vertex, None),
        ("shadow.frag", "shadow.frag", Fragment, None),
        ("shadow.frag", "shadow_mask.frag", Fragment, mask),
        ("sky.frag", "sky.frag", Fragment, None),
        ("irradiance.frag", "irradiance.frag", Fragment, None),
        ("prefilter.frag", "prefilter.frag", Fragment, None),
        ("brdf_lut.frag", "brdf_lut.frag", Fragment, None),
        ("skybox.vert", "skybox.vert", Vertex, None),
        ("skybox.frag", "skybox.frag", Fragment, None),
        ("tonemap.vert", "tonemap.vert", Vertex, None),
        ("tonemap.frag", "tonemap.frag", Fragment, None),
    ] {
        let path = format!("shaders/{file}");
        println!("cargo:rerun-if-changed={path}");
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut opts = make_options();
        if let Some(d) = define {
            opts.add_macro_definition(d, None);
        }
        let spv = compiler
            .compile_into_spirv(&src, kind, &path, "main", Some(&opts))
            .unwrap_or_else(|e| panic!("compile {path}: {e}"));
        fs::write(out.join(format!("{out_name}.spv")), spv.as_binary_u8()).expect("write spv");
    }
    println!("cargo:rerun-if-changed=shaders/common.glsl");
    for f in ["ibl", "bindings", "surface", "lighting"] {
        println!("cargo:rerun-if-changed=shaders/{f}.glsl");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
