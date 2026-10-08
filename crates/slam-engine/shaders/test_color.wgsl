// GPU tests: vertex colour times a tint from uniform slot 2.

struct Globals {
    tint: vec4<f32>,
}

@group(0) @binding(2) var<uniform> globals: Globals;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) colour: vec4<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) colour: vec4<f32>) -> VertexOutput {
    return VertexOutput(vec4<f32>(position, 0.0, 1.0), colour * globals.tint);
}

@fragment
fn fs_main(@location(0) colour: vec4<f32>) -> @location(0) vec4<f32> {
    return colour;
}
