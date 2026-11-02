//! Compiles the GLSL shaders to SPIR-V in `OUT_DIR` (no committed .spv files). The Vulkan 1.3
//! target environment is required for `GL_EXT_buffer_reference` and `nonuniformEXT`.

use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let compiler = shaderc::Compiler::new().expect("shaderc compiler");
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
    for (file, kind) in [
        ("forward.vert", shaderc::ShaderKind::Vertex),
        ("forward.frag", shaderc::ShaderKind::Fragment),
        ("shadow.vert", shaderc::ShaderKind::Vertex),
        ("shadow.frag", shaderc::ShaderKind::Fragment),
        ("sky.frag", shaderc::ShaderKind::Fragment),
        ("irradiance.frag", shaderc::ShaderKind::Fragment),
        ("prefilter.frag", shaderc::ShaderKind::Fragment),
        ("brdf_lut.frag", shaderc::ShaderKind::Fragment),
        ("skybox.vert", shaderc::ShaderKind::Vertex),
        ("skybox.frag", shaderc::ShaderKind::Fragment),
        ("tonemap.vert", shaderc::ShaderKind::Vertex),
        ("tonemap.frag", shaderc::ShaderKind::Fragment),
    ] {
        let path = format!("shaders/{file}");
        println!("cargo:rerun-if-changed={path}");
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let spv = compiler
            .compile_into_spirv(&src, kind, &path, "main", Some(&options))
            .unwrap_or_else(|e| panic!("compile {path}: {e}"));
        fs::write(out.join(format!("{file}.spv")), spv.as_binary_u8()).expect("write spv");
    }
    println!("cargo:rerun-if-changed=shaders/common.glsl");
    println!("cargo:rerun-if-changed=shaders/ibl.glsl");
    println!("cargo:rerun-if-changed=build.rs");
}
