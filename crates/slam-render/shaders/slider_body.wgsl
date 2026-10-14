// Slider body composite (ADR-0041): draws the slider's rectangle, reading the
// prepass value of each pixel from the R16Float target (`body_prepass`, the same
// size as the destination, pixel for pixel) and turning it into the gradient's
// colour, as osu-framework's sh_Path.fs with SmoothPath's gradient texture. The
// texture is replaced by texels in the uniform block, sampled the same way: linear
// between texel centres, clamped at the ends.

// Mirrors `BodyUniforms` in src/slider.rs; shared with slider_prepass.wgsl.
struct Body {
    projection: mat4x4<f32>,
    path: vec4<f32>,
    // Rectangle to draw in path coordinates: min x, min y, max x, max y.
    bounds: vec4<f32>,
    // Straight-alpha colour multiplying the result.
    tint: vec4<f32>,
    // x: texels in use.
    gradient: vec4<f32>,
    // RGBA bytes, r in the low byte, four texels per element.
    texels: array<vec4<u32>, 32>,
}

@group(0) @binding(0) var<uniform> body: Body;
@group(1) @binding(0) var body_prepass: texture_2d<f32>;
@group(2) @binding(0) var body_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// `corner`: 0 or 1 on each axis, from a shared unit quad.
@vertex
fn vs_main(@location(0) corner: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    let local = mix(body.bounds.xy, body.bounds.zw, corner);
    let clip = body.projection * vec4<f32>(local, 0.0, 1.0);
    out.position = clip;
    // Same pixel of the prepass target: v = 0 at its top (ADR-0019).
    let ndc = clip.xy / clip.w;
    out.uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    return out;
}

fn texel(i: u32) -> vec4<f32> {
    let packed = body.texels[i / 4u][i % 4u];
    return vec4<f32>(
        f32(packed & 0xffu),
        f32((packed >> 8u) & 0xffu),
        f32((packed >> 16u) & 0xffu),
        f32(packed >> 24u),
    ) / 255.0;
}

// Ported from osu-framework 2026.921.1: osu.Framework/Resources/Shaders/sh_Path.fs
@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let value = textureSample(body_prepass, body_sampler, uv).r;
    // As `SliderGradient::sample` in src/slider.rs.
    let width = body.gradient.x;
    let last = width - 1.0;
    let x = value * width - 0.5;
    let i0 = clamp(floor(x), 0.0, last);
    let i1 = min(i0 + 1.0, last);
    let t = clamp(x - i0, 0.0, 1.0);
    var colour = mix(texel(u32(i0)), texel(u32(i1)), t);
    if value <= 0.0 {
        colour.a = 0.0;
    }
    return body.tint * colour;
}
