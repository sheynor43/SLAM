// The `triangle` example: a rotated, aspect-corrected triangle with vertex colours.

struct Frame {
    // (cos, sin, x scale, unused)
    rotation_aspect: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) colour: vec4<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) colour: vec4<f32>) -> VertexOutput {
    let c = frame.rotation_aspect.x;
    let s = frame.rotation_aspect.y;
    let p = vec2<f32>(c * position.x - s * position.y, s * position.x + c * position.y);
    return VertexOutput(vec4<f32>(p.x * frame.rotation_aspect.z, p.y, 0.0, 1.0), colour);
}

@fragment
fn fs_main(@location(0) colour: vec4<f32>) -> @location(0) vec4<f32> {
    return colour;
}
