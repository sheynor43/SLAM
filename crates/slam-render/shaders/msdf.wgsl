// Multi-channel signed distance field text (ADR-0038): the sprite vertices and
// bindings of `sprite.wgsl`, with a glyph atlas of MSDF bitmaps instead of colour.
// The median of RGB is the distance to the outline, 0.5 on the edge and brighter
// inside; it is scaled to screen pixels and antialiased over about one pixel, as
// msdfgen recommends. The colour is straight alpha; the field only scales alpha.

struct Globals {
    // Maps positions to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var glyph_texture: texture_2d<f32>;
@group(2) @binding(0) var glyph_sampler: sampler;

// Total distance range of the field in atlas pixels; `slam_text::msdf::DISTANCE_RANGE`.
const DISTANCE_RANGE: f32 = 4.0;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = globals.projection * vec4<f32>(in.position, 0.0, 1.0);
    out.colour = in.colour;
    out.uv = in.uv;
    return out;
}

fn median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

@fragment
fn fs_main(@location(0) colour: vec4<f32>, @location(1) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let field = textureSample(glyph_texture, glyph_sampler, uv).rgb;
    // The field's range in UV units, then in screen pixels from the UV footprint of
    // a pixel (`fwidth` is the same on the surface and on render targets). Below one
    // pixel the edge would alias or vanish, so it is held there.
    let unit_range = vec2<f32>(DISTANCE_RANGE) / vec2<f32>(textureDimensions(glyph_texture, 0));
    // A degenerate quad (no UV change across a pixel) would divide by zero.
    let footprint = max(fwidth(uv), vec2<f32>(1e-6));
    let screen_range = max(0.5 * dot(unit_range, vec2<f32>(1.0) / footprint), 1.0);
    let distance = screen_range * (median(field.r, field.g, field.b) - 0.5);
    let coverage = clamp(distance + 0.5, 0.0, 1.0);
    return vec4<f32>(colour.rgb, colour.a * coverage);
}
