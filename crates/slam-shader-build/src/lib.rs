//! Build-time shader translation (ADR-0019). WGSL is parsed and validated by `naga`
//! and written out for each render backend together with the binding table the
//! backend needs. Called from `build.rs` only; nothing here reaches the runtime.
//!
//! Binding rules: `@group(0)` holds uniform buffers, `@group(1)` 2D textures and
//! `@group(2)` samplers; `@binding` is the HAL slot. A texture in slot `n` is sampled
//! only with the sampler in slot `n` (filtering is a property of the HAL texture).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use naga::back::glsl;
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};
use naga::{AddressSpace, Binding as IoBinding, BuiltIn, ImageClass, ImageDimension, Module};
use naga::{ShaderStage, TypeInner};

pub const UNIFORM_GROUP: u32 = 0;
pub const TEXTURE_GROUP: u32 = 1;
pub const SAMPLER_GROUP: u32 = 2;
pub const VERTEX_ENTRY: &str = "vs_main";
pub const FRAGMENT_ENTRY: &str = "fs_main";

/// Mirrors `slam_render::BindingKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingKind {
    UniformBuffer,
    Texture,
}

/// A generated GLSL resource name and the HAL slot it is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub kind: BindingKind,
    pub slot: u32,
}

/// GLSL 3.30 core output.
#[derive(Clone, Debug)]
pub struct Glsl {
    /// Vertex stage for drawing to the surface: GL already shows clip-space y up.
    pub vertex_surface: String,
    /// Vertex stage for drawing to a render target: y is flipped so that the first
    /// row of the target's texture is the top of the picture.
    pub vertex_target: String,
    pub fragment: String,
    /// Sorted by kind, slot and name. A slot may appear under several names (one per
    /// stage).
    pub bindings: Vec<Binding>,
}

#[derive(Clone, Debug)]
pub struct Shader {
    /// The file stem.
    pub name: String,
    pub glsl: Glsl,
}

/// A shader that failed to translate. The message names the file.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("shader `{}`: {}", path.display(), message.trim_end())]
pub struct ShaderError {
    pub path: PathBuf,
    pub message: String,
}

/// Translates one WGSL shader. `path` names it in errors; its stem is the shader name.
pub fn translate(path: &Path, source: &str) -> Result<Shader, ShaderError> {
    let error = |message: String| ShaderError {
        path: path.to_owned(),
        message,
    };
    let label = path.display().to_string();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| const_name(s).is_some())
        .ok_or_else(|| error("file name must be an ASCII identifier".into()))?;

    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| error(e.emit_to_string_with_path(source, &label)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| error(e.emit_to_string_with_path(source, &label)))?;
    check_entry_points(&module).map_err(error)?;
    check_globals(&module).map_err(error)?;

    let surface = write_glsl(&module, &info, ShaderStage::Vertex, false).map_err(error)?;
    let target = write_glsl(&module, &info, ShaderStage::Vertex, true).map_err(error)?;
    let fragment = write_glsl(&module, &info, ShaderStage::Fragment, false).map_err(error)?;

    let mut bindings = Vec::new();
    for reflection in [&surface.1, &fragment.1] {
        collect_bindings(&module, reflection, &mut bindings).map_err(error)?;
    }
    bindings.sort_by(|a, b| (a.kind, a.slot, &a.name).cmp(&(b.kind, b.slot, &b.name)));

    Ok(Shader {
        name: name.to_owned(),
        glsl: Glsl {
            vertex_surface: surface.0,
            vertex_target: target.0,
            fragment: fragment.0,
            bindings,
        },
    })
}

/// Translates every `*.wgsl` file in `dir` (sorted by name). All failures are
/// reported, not just the first.
pub fn translate_dir(dir: &Path) -> Result<Vec<Shader>, Vec<ShaderError>> {
    let paths = shader_paths(dir).map_err(|message| {
        vec![ShaderError {
            path: dir.to_owned(),
            message,
        }]
    })?;
    let mut shaders = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let result = fs::read_to_string(&path)
            .map_err(|e| ShaderError {
                path: path.clone(),
                message: e.to_string(),
            })
            .and_then(|source| translate(&path, &source));
        match result {
            Ok(shader) => shaders.push(shader),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(shaders)
    } else {
        Err(errors)
    }
}

/// Rust source with one `pub const NAME: Shader<'static>` per shader and `ALL`.
/// `hal` is the path to the HAL types: `crate` inside `slam-render`, `slam_render`
/// elsewhere.
pub fn generate(shaders: &[Shader], hal: &str) -> String {
    let mut out = String::from("// Generated by slam-shader-build. Do not edit.\n\n");
    for shader in shaders {
        let glsl = &shader.glsl;
        let ident = const_name(&shader.name).expect("validated by translate");
        let _ = writeln!(
            out,
            "pub const {ident}: {hal}::Shader<'static> = {hal}::Shader {{"
        );
        let _ = writeln!(out, "    name: {:?},", shader.name);
        let _ = writeln!(out, "    glsl: {hal}::GlslShader {{");
        let _ = writeln!(out, "        vertex_surface: {:?},", glsl.vertex_surface);
        let _ = writeln!(out, "        vertex_target: {:?},", glsl.vertex_target);
        let _ = writeln!(out, "        fragment: {:?},", glsl.fragment);
        let _ = writeln!(out, "        bindings: &[");
        for b in &glsl.bindings {
            let _ = writeln!(
                out,
                "            {hal}::Binding {{ name: {:?}, kind: {hal}::BindingKind::{:?}, slot: {} }},",
                b.name, b.kind, b.slot
            );
        }
        let _ = writeln!(out, "        ],\n    }},\n}};\n");
    }
    let _ = write!(out, "pub const ALL: &[&{hal}::Shader<'static>] = &[");
    for shader in shaders {
        let _ = write!(out, "&{}, ", const_name(&shader.name).expect("validated"));
    }
    out.push_str("];\n");
    out
}

/// Build script entry: translates `shaders_dir` (relative to the crate) and writes
/// `$OUT_DIR/shaders.rs`. On failure prints every error and exits the build.
pub fn build(shaders_dir: &str, hal: &str) {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("run from build.rs");
    let out_dir = env::var_os("OUT_DIR").expect("run from build.rs");
    let dir = Path::new(&manifest).join(shaders_dir);
    println!("cargo::rerun-if-changed={}", dir.display());
    if let Ok(paths) = shader_paths(&dir) {
        for path in paths {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    match translate_dir(&dir) {
        Ok(shaders) => {
            let path = Path::new(&out_dir).join("shaders.rs");
            if let Err(e) = fs::write(&path, generate(&shaders, hal)) {
                eprintln!("cannot write {}: {e}", path.display());
                std::process::exit(1);
            }
        }
        Err(errors) => {
            for e in errors {
                eprintln!("error: {e}\n");
            }
            std::process::exit(1);
        }
    }
}

fn shader_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read directory: {e}"))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// `sprite-batch` → `SPRITE_BATCH`; `None` unless the stem is an ASCII identifier
/// (letters, digits, `_`, `-`; not starting with a digit).
fn const_name(stem: &str) -> Option<String> {
    let valid = stem
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let ident: String = stem
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    (valid && ident != "ALL" && ident.chars().any(|c| c != '_')).then_some(ident)
}

fn check_entry_points(module: &Module) -> Result<(), String> {
    let expected = [
        (VERTEX_ENTRY, ShaderStage::Vertex),
        (FRAGMENT_ENTRY, ShaderStage::Fragment),
    ];
    for ep in &module.entry_points {
        if !expected.contains(&(ep.name.as_str(), ep.stage)) {
            return Err(format!(
                "unexpected entry point `{}` ({:?}); expected `@vertex fn {VERTEX_ENTRY}` and `@fragment fn {FRAGMENT_ENTRY}`",
                ep.name, ep.stage
            ));
        }
    }
    for (name, stage) in expected {
        if !module.entry_points.iter().any(|ep| ep.name == name) {
            return Err(format!("missing {stage:?} entry point `{name}`"));
        }
    }
    let fragment = module
        .entry_points
        .iter()
        .find(|ep| ep.stage == ShaderStage::Fragment)
        .expect("checked above");
    // OpenGL flips y for render targets only, so anything that sees the window's y
    // direction or the winding differs between the surface and render targets.
    for arg in &fragment.function.arguments {
        let mut builtins = vec![arg.binding.as_ref()];
        if let TypeInner::Struct { members, .. } = &module.types[arg.ty].inner {
            builtins.extend(members.iter().map(|m| m.binding.as_ref()));
        }
        for binding in builtins {
            let name = match binding {
                Some(IoBinding::BuiltIn(BuiltIn::Position { .. })) => "position",
                Some(IoBinding::BuiltIn(BuiltIn::FrontFacing)) => "front_facing",
                _ => continue,
            };
            return Err(format!(
                "`{FRAGMENT_ENTRY}` reads `@builtin({name})`, which differs between the surface and render targets in OpenGL; pass values through a `@location` instead"
            ));
        }
    }
    let functions = module.functions.iter().map(|(_, f)| f);
    for function in functions.chain(std::iter::once(&fragment.function)) {
        let reads_dpdy = function.expressions.iter().any(|(_, e)| {
            matches!(
                e,
                naga::Expression::Derivative {
                    axis: naga::DerivativeAxis::Y,
                    ..
                }
            )
        });
        if reads_dpdy {
            return Err(
                "`dpdy` changes sign between the surface and render targets in OpenGL; use `fwidth` or derive from a `@location` instead".into(),
            );
        }
    }
    Ok(())
}

fn check_globals(module: &Module) -> Result<(), String> {
    for (_, var) in module.global_variables.iter() {
        let Some(binding) = &var.binding else {
            continue;
        };
        let name = var.name.as_deref().unwrap_or("<unnamed>");
        let (group, what) = match (var.space, &module.types[var.ty].inner) {
            (AddressSpace::Uniform, _) => (UNIFORM_GROUP, "uniform buffer"),
            (
                AddressSpace::Handle,
                TypeInner::Image {
                    dim: ImageDimension::D2,
                    arrayed: false,
                    class:
                        ImageClass::Sampled {
                            kind: naga::ScalarKind::Float,
                            multi: false,
                        },
                },
            ) => (TEXTURE_GROUP, "texture"),
            (AddressSpace::Handle, TypeInner::Sampler { comparison: false }) => {
                (SAMPLER_GROUP, "sampler")
            }
            _ => {
                return Err(format!(
                    "`{name}`: only uniform buffers, `texture_2d<f32>` and `sampler` are supported"
                ));
            }
        };
        if binding.group != group {
            return Err(format!(
                "{what} `{name}` is in @group({}); {what}s belong in @group({group})",
                binding.group
            ));
        }
        let has_texture = || {
            module.global_variables.iter().any(|(_, v)| {
                v.binding
                    .as_ref()
                    .is_some_and(|b| b.group == TEXTURE_GROUP && b.binding == binding.binding)
            })
        };
        if group == SAMPLER_GROUP && !has_texture() {
            return Err(format!(
                "sampler `{name}` (@binding({})) has no texture at the same @binding",
                binding.binding
            ));
        }
    }
    Ok(())
}

fn write_glsl(
    module: &Module,
    info: &ModuleInfo,
    stage: ShaderStage,
    flip_y: bool,
) -> Result<(String, glsl::ReflectionInfo), String> {
    let options = glsl::Options {
        version: glsl::Version::Desktop(330),
        writer_flags: if flip_y {
            glsl::WriterFlags::ADJUST_COORDINATE_SPACE
        } else {
            glsl::WriterFlags::empty()
        },
        ..Default::default()
    };
    let entry_point = match stage {
        ShaderStage::Vertex => VERTEX_ENTRY,
        _ => FRAGMENT_ENTRY,
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: stage,
        entry_point: entry_point.into(),
        multiview: None,
    };
    let mut source = String::new();
    let reflection = glsl::Writer::new(
        &mut source,
        module,
        info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .and_then(|mut writer| writer.write())
    .map_err(|e| format!("GLSL output of `{entry_point}`: {e}"))?;
    Ok((source, reflection))
}

fn collect_bindings(
    module: &Module,
    reflection: &glsl::ReflectionInfo,
    out: &mut Vec<Binding>,
) -> Result<(), String> {
    let slot = |handle: naga::Handle<naga::GlobalVariable>| {
        let var = &module.global_variables[handle];
        let binding = var.binding.as_ref().expect("resources have bindings");
        (var.name.as_deref().unwrap_or("<unnamed>"), binding.binding)
    };
    let mut push = |name: &str, kind, slot| {
        if !out.iter().any(|b| b.name == name) {
            out.push(Binding {
                name: name.to_owned(),
                kind,
                slot,
            });
        }
    };
    for (&handle, name) in &reflection.uniforms {
        push(name, BindingKind::UniformBuffer, slot(handle).1);
    }
    for (name, mapping) in &reflection.texture_mapping {
        let (texture, texture_slot) = slot(mapping.texture);
        if let Some(sampler) = mapping.sampler {
            let (sampler, sampler_slot) = slot(sampler);
            if sampler_slot != texture_slot {
                return Err(format!(
                    "texture `{texture}` (@binding({texture_slot})) is sampled with `{sampler}` (@binding({sampler_slot})); a texture's sampler must have the same @binding"
                ));
            }
        }
        push(name, BindingKind::Texture, texture_slot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPRITE: &str = r#"
struct Globals { proj: mat4x4<f32> }
@group(0) @binding(1) var<uniform> globals: Globals;
@group(1) @binding(2) var tex: texture_2d<f32>;
@group(2) @binding(2) var samp: sampler;
struct VsIn { @location(0) pos: vec2<f32>, @location(1) uv: vec2<f32> }
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(v: VsIn) -> VsOut {
    return VsOut(globals.proj * vec4(v.pos, 0.0, 1.0), v.uv);
}
@fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, uv) * globals.proj[0][0];
}
"#;

    fn translate_str(source: &str) -> Result<Shader, ShaderError> {
        translate(Path::new("shaders/test.wgsl"), source)
    }

    fn message(source: &str) -> String {
        translate_str(source).unwrap_err().to_string()
    }

    #[test]
    fn translates_to_glsl_330_with_bindings() {
        let shader = translate_str(SPRITE).unwrap();
        let glsl = &shader.glsl;
        assert_eq!(shader.name, "test");
        for source in [&glsl.vertex_surface, &glsl.vertex_target, &glsl.fragment] {
            assert!(source.starts_with("#version 330 core\n"), "{source}");
        }
        assert!(glsl.vertex_surface.contains("layout(location = 1) in vec2"));
        let kinds: Vec<_> = glsl.bindings.iter().map(|b| (b.kind, b.slot)).collect();
        // The uniform block is used in both stages and has a name per stage.
        assert_eq!(
            kinds,
            [
                (BindingKind::UniformBuffer, 1),
                (BindingKind::UniformBuffer, 1),
                (BindingKind::Texture, 2)
            ]
        );
        for b in &glsl.bindings {
            let in_vertex = glsl.vertex_surface.contains(&b.name);
            let in_fragment = glsl.fragment.contains(&b.name);
            assert!(in_vertex || in_fragment, "{b:?} not in output");
        }
    }

    #[test]
    fn only_render_target_variant_flips_y() {
        let glsl = translate_str(SPRITE).unwrap().glsl;
        let flip = "gl_Position.yz = vec2(-gl_Position.y";
        assert!(glsl.vertex_target.contains(flip));
        assert!(!glsl.vertex_surface.contains(flip));
        assert!(!glsl.fragment.contains(flip));
        assert_eq!(
            glsl.vertex_surface,
            glsl.vertex_target.replace(
                "    gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);\n",
                ""
            )
        );
    }

    #[test]
    fn errors_name_the_file_and_line() {
        let msg = message("@vertex fn vs_main() -> @builtin(position) vec4<f32> { return 1; }");
        assert!(msg.starts_with("shader `shaders/test.wgsl`: "), "{msg}");
        assert!(msg.contains("shaders/test.wgsl:1:"), "{msg}");
        let msg = message("fn broken( {");
        assert!(msg.starts_with("shader `shaders/test.wgsl`: "), "{msg}");
    }

    #[test]
    fn requires_both_entry_points() {
        let vs = "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4(0.0); }";
        let fs = "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4(1.0); }";
        assert!(message(vs).contains("missing Fragment entry point `fs_main`"));
        assert!(message(fs).contains("missing Vertex entry point `vs_main`"));
        let renamed = format!("{vs}\n{}", fs.replace("fs_main", "main"));
        assert!(message(&renamed).contains("unexpected entry point `main`"));
        assert!(translate_str(&format!("{vs}\n{fs}")).is_ok());
    }

    #[test]
    fn rejects_position_in_fragment_input() {
        let source = SPRITE.replace(
            "fn fs_main(@location(0) uv: vec2<f32>)",
            "fn fs_main(v: VsOut)",
        );
        let source = source.replace(
            "textureSample(tex, samp, uv)",
            "textureSample(tex, samp, v.uv)",
        );
        assert!(message(&source).contains("reads `@builtin(position)`"));
    }

    #[test]
    fn enforces_groups() {
        let msg = message(&SPRITE.replace("@group(0) @binding(1)", "@group(3) @binding(1)"));
        assert!(
            msg.contains("uniform buffer `globals` is in @group(3)"),
            "{msg}"
        );
        let msg = message(&SPRITE.replace("@group(1) @binding(2)", "@group(0) @binding(2)"));
        assert!(msg.contains("texture `tex` is in @group(0)"), "{msg}");
        let msg = message(&SPRITE.replace("@group(2) @binding(2)", "@group(1) @binding(3)"));
        assert!(msg.contains("sampler `samp` is in @group(1)"), "{msg}");
    }

    #[test]
    fn sampler_slot_must_match_texture_slot() {
        // A texture in slot 0 exists, but `tex` (slot 2) is sampled with slot 0's sampler.
        let source = SPRITE.replace(
            "@group(2) @binding(2)",
            "@group(1) @binding(0) var other: texture_2d<f32>;\n@group(2) @binding(0)",
        );
        let msg = message(&source);
        assert!(
            msg.contains("texture `tex` (@binding(2)) is sampled with `samp` (@binding(0))"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_unsupported_resources() {
        let source = SPRITE
            .replace("var tex: texture_2d<f32>", "var tex: texture_2d_array<f32>")
            .replace(
                "textureSample(tex, samp, uv)",
                "textureSample(tex, samp, uv, 0)",
            );
        assert!(message(&source).contains("only uniform buffers, `texture_2d<f32>` and `sampler`"));
        let source = SPRITE
            .replace("texture_2d<f32>", "texture_2d<u32>")
            .replace(
                "textureSample(tex, samp, uv)",
                "vec4<f32>(textureLoad(tex, vec2(0), 0))",
            );
        assert!(message(&source).contains("only uniform buffers, `texture_2d<f32>` and `sampler`"));
    }

    #[test]
    fn rejects_y_dependent_fragment_inputs() {
        let source = SPRITE.replace(
            "fn fs_main(@location(0) uv: vec2<f32>)",
            "fn fs_main(@location(0) uv: vec2<f32>, @builtin(front_facing) front: bool)",
        );
        assert!(message(&source).contains("reads `@builtin(front_facing)`"));
        let helper = "fn slope(x: f32) -> f32 { return dpdy(x); }\n";
        let source = format!(
            "{helper}{}",
            SPRITE.replace("globals.proj[0][0]", "slope(uv.x)")
        );
        assert!(message(&source).contains("`dpdy` changes sign"));
        let source = SPRITE.replace("globals.proj[0][0]", "fwidth(uv.x) + dpdx(uv.x)");
        assert!(translate_str(&source).is_ok());
    }

    #[test]
    fn sampler_needs_a_texture_in_its_slot() {
        let source = SPRITE
            .replace(
                "@group(2) @binding(2) var samp",
                "@group(2) @binding(5) var samp",
            )
            .replace(
                "textureSample(tex, samp, uv)",
                "textureLoad(tex, vec2(0), 0)",
            );
        let msg = message(&source);
        assert!(
            msg.contains("sampler `samp` (@binding(5)) has no texture"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_bad_file_names() {
        for name in ["1st.wgsl", "a b.wgsl", "all.wgsl", ".wgsl", "--.wgsl"] {
            let err = translate(Path::new(name), SPRITE).unwrap_err();
            assert!(err.message.contains("ASCII identifier"), "{name}");
        }
        assert_eq!(const_name("sprite-batch").as_deref(), Some("SPRITE_BATCH"));
    }

    #[test]
    fn generates_constants() {
        let shader = translate(Path::new("sprite-batch.wgsl"), SPRITE).unwrap();
        let code = generate(&[shader], "slam_render");
        assert!(code.contains(
            "pub const SPRITE_BATCH: slam_render::Shader<'static> = slam_render::Shader {"
        ));
        assert!(code.contains("name: \"sprite-batch\","));
        assert!(code.contains("kind: slam_render::BindingKind::Texture, slot: 2 }"));
        assert!(
            code.contains("pub const ALL: &[&slam_render::Shader<'static>] = &[&SPRITE_BATCH, ];")
        );
        assert!(code.contains("\\n"), "sources are escaped string literals");
    }

    #[test]
    fn translate_dir_reports_every_failure() {
        let dir = std::env::temp_dir().join(format!("slam-shader-build-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("good.wgsl"), SPRITE).unwrap();
        fs::write(dir.join("bad_a.wgsl"), "fn (").unwrap();
        fs::write(dir.join("bad_b.wgsl"), "fn (").unwrap();
        fs::write(dir.join("notes.txt"), "not a shader").unwrap();
        let errors = translate_dir(&dir).unwrap_err();
        let names: Vec<_> = errors.iter().map(|e| e.path.file_name().unwrap()).collect();
        assert_eq!(names, ["bad_a.wgsl", "bad_b.wgsl"]);
        fs::remove_file(dir.join("bad_a.wgsl")).unwrap();
        fs::remove_file(dir.join("bad_b.wgsl")).unwrap();
        let shaders = translate_dir(&dir).unwrap();
        assert_eq!(shaders.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
