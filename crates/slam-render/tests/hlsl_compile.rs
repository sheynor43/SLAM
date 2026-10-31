//! The generated HLSL compiles to Shader Model 5.0 with `D3DCompile`, as the D3D11
//! backend does when it creates a pipeline (ADR-0080).
#![cfg(windows)]

use slam_render::shaders;
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::core::PCSTR;

fn blob_text(blob: &ID3DBlob) -> String {
    // SAFETY: reading the size of a live blob.
    if unsafe { blob.GetBufferSize() } == 0 {
        return String::new();
    }
    // SAFETY: the blob owns `GetBufferSize` bytes at `GetBufferPointer` while alive.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    String::from_utf8_lossy(bytes).into_owned()
}

fn compile(name: &str, source: &str, entry: &str, target: &str) -> Result<usize, String> {
    let entry = format!("{entry}\0");
    let target = format!("{target}\0");
    let mut code = None;
    let mut errors = None;
    // SAFETY: `source` outlives the call, the strings are NUL-terminated and both
    // out-pointers point to live `Option<ID3DBlob>`s.
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match (result, code) {
        // SAFETY: as in `blob_text`.
        (Ok(()), Some(code)) => Ok(unsafe { code.GetBufferSize() }),
        (result, _) => Err(format!(
            "{name} ({target}): {result:?}: {}",
            errors.as_ref().map(blob_text).unwrap_or_default()
        )),
    }
}

#[test]
fn every_shader_compiles_to_shader_model_5() {
    let mut failures = Vec::new();
    for shader in shaders::ALL {
        let hlsl = shader.hlsl;
        for (entry, target) in [
            (hlsl.vertex_entry, "vs_5_0"),
            (hlsl.fragment_entry, "ps_5_0"),
        ] {
            match compile(shader.name, hlsl.source, entry, target) {
                Ok(len) => assert!(len > 0, "{}: empty bytecode", shader.name),
                Err(e) => failures.push(e),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
