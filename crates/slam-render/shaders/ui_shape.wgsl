// Interface shape (ADR-0116): a rounded rectangle, filled or outlined, optionally
// skewed, tinting a texture region and lit by a pattern of triangles. The skew is in the
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

// Brightening in [0, 1] of the triangle under `p` (pixels): rows of triangles a cell
// high, every other row shifted half a cell, drifting up with `phase` (cells). Most
// triangles stay dark, so the pattern reads as scattered shapes.
fn triangles(p: vec2<f32>, cell: f32, phase: f32) -> f32 {
    let g = vec2<f32>(p.x / cell, p.y / cell + phase);
    let row = floor(g.y);
    let fy = fract(g.y);
    let q = g.x + 0.5 * row;
    let column = floor(q);
    let fx = fract(q);
    let upper = select(0.0, 1.0, fx + fy > 1.0);
    let h = hash(vec3<f32>(column, row, upper));
    return max(h - 0.55, 0.0) / 0.45;
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
    let outline = params.y;
    let pattern = params.z;
    let d = rounded_box(local, half_size, radius);
    let aa = max(fwidth(d), 1e-4);
    var coverage = clamp(0.5 - d / aa, 0.0, 1.0);
    if (outline > 0.0) {
        coverage = coverage * clamp(0.5 + (d + outline) / aa, 0.0, 1.0);
    }
    var out = colour * textureSample(shape_texture, shape_sampler, uv);
    if (pattern > 0.0) {
        let cell = max(half_size.y * 0.9, 6.0);
        let light = triangles(local + half_size, cell, params.w) * pattern;
        out = vec4<f32>(mix(out.rgb, vec3<f32>(1.0, 1.0, 1.0), light), out.a);
    }
    return vec4<f32>(out.rgb, out.a * coverage);
}
