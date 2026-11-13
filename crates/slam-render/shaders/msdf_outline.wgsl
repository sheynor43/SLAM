// Outline of multi-channel signed distance field text (ADR-0114): the vertices,
// bindings and field decoding of `msdf.wgsl`, with a band of coverage centred on the
// glyph's edge instead of its inside. The band is `STROKE` atlas pixels (at
// `slam_text::EM_PX`) wide, so it grows with the text, and not thinner than one screen
// pixel while the field's range allows. The colour is straight alpha; the band only
// scales alpha.

struct Globals {
    // Maps positions to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var glyph_texture: texture_2d<f32>;
@group(2) @binding(0) var glyph_sampler: sampler;

// Total distance range of the field in atlas pixels; `slam_text::msdf::DISTANCE_RANGE`.
const DISTANCE_RANGE: f32 = 4.0;
// Width of the band in atlas pixels. With its antialiasing it must stay inside the
// field's range (`DISTANCE_RANGE`), where distances are known.
const STROKE: f32 = 1.2;

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
    // As in `msdf.wgsl`: the field's range in screen pixels, at least one.
    let unit_range = vec2<f32>(DISTANCE_RANGE) / vec2<f32>(textureDimensions(glyph_texture, 0));
    let footprint = max(fwidth(uv), vec2<f32>(1e-6));
    let screen_range = max(0.5 * dot(unit_range, vec2<f32>(1.0) / footprint), 1.0);
    let distance = screen_range * (median(field.r, field.g, field.b) - 0.5);
    // Half the band in screen pixels, from atlas pixels, at least half a pixel. With its
    // antialiasing it must end where the field still holds distances (`screen_range / 2`
    // from the edge): beyond, the field saturates and the band would cover the whole quad.
    // Text below about half `EM_PX` gets a thinner, fainter band instead.
    let room = 0.5 * screen_range - 0.5;
    let half_band = min(max(0.5 * STROKE * screen_range / DISTANCE_RANGE, 0.5), room);
    let coverage = clamp(half_band - abs(distance) + 0.5, 0.0, 1.0);
    return vec4<f32>(colour.rgb, colour.a * coverage);
}
