//! The test and example shaders compile to Shader Model 5.0 with `D3DCompile`, as the
//! D3D11 backend does when it creates a pipeline (ADR-0080).
#![cfg(windows)]

use slam_render::d3d11::compile_shader;

// Generated from `shaders/*.wgsl` by the build script.
#[allow(dead_code)]
mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}

#[test]
fn every_shader_compiles_to_shader_model_5() {
    let failures: Vec<String> = shaders::ALL
        .iter()
        .filter_map(|shader| compile_shader(shader).err())
        .map(|e| e.to_string())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
