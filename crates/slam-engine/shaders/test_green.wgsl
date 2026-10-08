// GPU tests: the `test_color` vertex input, always green.

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) colour: vec4<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
