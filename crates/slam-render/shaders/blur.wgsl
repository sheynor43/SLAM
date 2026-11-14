// One pass of a separable Gaussian blur (ADR-0123): nine taps along the step the vertex
// colour carries in its red and green (texture coordinates per tap; the colour tints
// nothing here). Drawn twice, across and then down, into render targets.

struct Globals {
    // Maps positions to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var blur_texture: texture_2d<f32>;
@group(2) @binding(0) var blur_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) step: vec2<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = globals.projection * vec4<f32>(in.position, 0.0, 1.0);
    out.step = in.colour.xy;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(@location(0) step: vec2<f32>, @location(1) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // Weights of a Gaussian with a sigma of two taps, summing to one.
    var sum = textureSample(blur_texture, blur_sampler, uv) * 0.2270270;
    sum = sum + (textureSample(blur_texture, blur_sampler, uv + step)
        + textureSample(blur_texture, blur_sampler, uv - step)) * 0.1945946;
    sum = sum + (textureSample(blur_texture, blur_sampler, uv + 2.0 * step)
        + textureSample(blur_texture, blur_sampler, uv - 2.0 * step)) * 0.1216216;
    sum = sum + (textureSample(blur_texture, blur_sampler, uv + 3.0 * step)
        + textureSample(blur_texture, blur_sampler, uv - 3.0 * step)) * 0.0540540;
    sum = sum + (textureSample(blur_texture, blur_sampler, uv + 4.0 * step)
        + textureSample(blur_texture, blur_sampler, uv - 4.0 * step)) * 0.0162162;
    return sum;
}
