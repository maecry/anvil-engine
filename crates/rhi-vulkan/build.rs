//! Compiles the integration-test GLSL shaders to SPIR-V in `OUT_DIR` (Vulkan 1.3 target so
//! `GL_EXT_buffer_reference` and `nonuniformEXT` compile).

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
    for (file, kind) in [
        ("sample.vert", shaderc::ShaderKind::Vertex),
        ("sample.frag", shaderc::ShaderKind::Fragment),
        ("flat.vert", shaderc::ShaderKind::Vertex),
        ("flat.frag", shaderc::ShaderKind::Fragment),
        ("empty.frag", shaderc::ShaderKind::Fragment),
    ] {
        let path = format!("tests/shaders/{file}");
        println!("cargo:rerun-if-changed={path}");
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let spv = compiler
            .compile_into_spirv(&src, kind, &path, "main", Some(&options))
            .unwrap_or_else(|e| panic!("compile {path}: {e}"));
        fs::write(out.join(format!("{file}.spv")), spv.as_binary_u8()).expect("write spv");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
