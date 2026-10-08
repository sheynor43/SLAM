// GPU tests: a texture from slot 3.

@group(1) @binding(3) var image: texture_2d<f32>;
@group(2) @binding(3) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    return VertexOutput(vec4<f32>(position, 0.0, 1.0), uv);
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, uv);
}
