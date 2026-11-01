//! The generated HLSL compiles to Shader Model 5.0 with `D3DCompile`, as the D3D11
//! backend does when it creates a pipeline (ADR-0080).
#![cfg(windows)]

use slam_render::d3d11::compile_shader;
use slam_render::shaders;

#[test]
fn every_shader_compiles_to_shader_model_5() {
    let mut failures = Vec::new();
    for shader in shaders::ALL {
        match compile_shader(shader) {
            Ok(code) => assert!(
                !code.vertex().is_empty() && !code.pixel().is_empty(),
                "{}: empty bytecode",
                shader.name
            ),
            Err(e) => failures.push(e.to_string()),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
