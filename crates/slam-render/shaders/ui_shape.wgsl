// Interface shape (ADR-0116, ADR-0123): a rounded rectangle, filled or outlined, optionally
// skewed, tinting a texture region, with a band of light sweeping across it; or the soft,
// pulsing glow around such a rectangle. The skew is in the
// geometry (`src/shape.rs` shears the corners); `local` stays unsheared, so the distance
// below is that of a plain rounded rectangle. Colour is straight alpha (ADR-0020).

struct Globals {
    // Maps positions to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var shape_texture: texture_2d<f32>;
@group(2) @binding(0) var shape_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
    // Pixels from the shape's centre, before the skew.
    @location(3) local: vec2<f32>,
    // Half the width and height in pixels.
    @location(4) half_size: vec2<f32>,
    // Corner radius, outline width (0: filled), pattern strength, pattern phase.
    @location(5) params: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) params: vec4<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = globals.projection * vec4<f32>(in.position, 0.0, 1.0);
    out.colour = in.colour;
    out.uv = in.uv;
    out.local = in.local;
    out.half_size = in.half_size;
    out.params = in.params;
    return out;
}

// Signed distance to a rectangle of half size `half_size` with corners rounded by
// `radius`: negative inside.
fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius, radius);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn hash(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q = q + dot(q, q.yxz + vec3<f32>(33.33, 33.33, 33.33));
    return fract((q.x + q.y) * q.z);
}

@fragment
fn fs_main(
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) params: vec4<f32>,
) -> @location(0) vec4<f32> {
    let radius = params.x;
    // Positive: an outline that wide; negative: a glow that wide outside the edge.
    let edge = params.y;
    // A fill's sheen strength, a glow's pulse depth.
    let amount = params.z;
    // Cycles of the sheen's sweep or the glow's pulse.
    let phase = params.w;
    let d = rounded_box(local, half_size, radius);
    let aa = max(fwidth(d), 1e-4);
    let texel = colour * textureSample(shape_texture, shape_sampler, uv);
    if (edge < 0.0) {
        let width = -edge;
        let outside = clamp(0.5 + d / aa, 0.0, 1.0);
        let fall = 1.0 - clamp(d / width, 0.0, 1.0);
        let pulse = 1.0 - amount * 0.5 * (1.0 + sin(6.2831853 * phase));
        // The colour alone: the quad reaches past the texture's region by the glow, where
        // the atlas holds other images or nothing.
        return vec4<f32>(colour.rgb, colour.a * outside * fall * fall * pulse);
    }
    var coverage = clamp(0.5 - d / aa, 0.0, 1.0);
    if (edge > 0.0) {
        coverage = coverage * clamp(0.5 + (d + edge) / aa, 0.0, 1.0);
    }
    var out = texel;
    if (amount > 0.0) {
        // A band leaning like the skew, from left of the shape to right of it once a cycle,
        // out of sight for the rest.
        let span = 2.0 * half_size.x + 1.2 * half_size.y;
        let across = (local.x + 0.6 * local.y) / span + 0.5;
        let centre = fract(phase) * 1.8 - 0.4;
        let band = exp(-pow((across - centre) / 0.07, 2.0));
        out = vec4<f32>(mix(out.rgb, vec3<f32>(1.0, 1.0, 1.0), band * amount), out.a);
    }
    return vec4<f32>(out.rgb, out.a * coverage);
}
