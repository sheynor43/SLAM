// Interface shape (ADR-0116, ADR-0123): a rounded rectangle, filled or outlined, optionally
// skewed, tinting a texture region, with a band of light sweeping across it and sparse
// triangles and circles floating up it (ADR-0131); or the soft, pulsing glow around such a
// rectangle. The skew is in the
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
    // Corner radius, outline width (0: filled), the packed effect word (8 bits each of sheen,
    // pattern strength and pattern seed; `pack_effects` in `src/shape.rs`), phase in cycles.
    @location(5) params: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    // Flat: the packed effect word is an integer that interpolation would blur.
    @location(4) @interpolate(flat, either) params: vec4<f32>,
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

// Signed distance in the units of `p` to an upward-pointing equilateral triangle whose
// corners are `r` from its centre (y down).
fn triangle(p: vec2<f32>, r: f32) -> f32 {
    let base = p.y - r * 0.5;
    let side = abs(p.x) * 0.8660254 - p.y * 0.5 - r * 0.5;
    return max(base, side);
}

// Brightening in [0, 1] of the sparse triangles and circles under `local` (pixels from the
// shape's centre): a grid of cells scrolling up with `phase`, each cell holding at most one
// small shape at a hashed spot, size and kind, fading towards the shape's top and bottom.
// `strength` sets how many there are and how fast they rise; `seed` differs per shape.
fn pattern(local: vec2<f32>, half_size: vec2<f32>, strength: f32, seed: f32, phase: f32) -> f32 {
    // Capped, so a huge panel does not get huge shapes.
    let cell = clamp(half_size.y * 1.5, 12.0, 64.0);
    // A multiple of 1/16: the drift wraps every 1024 cycles (`set_drift`), and 1024 times
    // the rate is a whole number of cells, so the pattern does not jump at the wrap.
    let rate = floor((1.5 + 6.0 * strength) * 16.0 + 0.5) / 16.0;
    let g = vec2<f32>((local.x + half_size.x) / cell, (local.y + half_size.y) / cell + phase * rate);
    let id = floor(g);
    let f = fract(g);
    let key = vec3<f32>(id.x + seed * 13.0, id.y, 0.0);
    if (hash(key) > 0.18 + 0.3 * strength) {
        return 0.0;
    }
    let radius = (0.10 + 0.16 * hash(key + vec3<f32>(0.0, 0.0, 3.0))) * cell;
    // The centre keeps the whole shape (a triangle's corners are 1.25 radii out, plus a
    // pixel of edge) inside the cell, which would clip it.
    let margin = min((radius * 1.25 + 1.0) / cell, 0.5);
    let centre = vec2<f32>(
        margin + (1.0 - 2.0 * margin) * hash(key + vec3<f32>(0.0, 0.0, 1.0)),
        margin + (1.0 - 2.0 * margin) * hash(key + vec3<f32>(0.0, 0.0, 2.0)),
    );
    let p = (f - centre) * cell;
    var d = length(p) - radius;
    if (hash(key + vec3<f32>(0.0, 0.0, 4.0)) < 0.5) {
        d = triangle(p, radius * 1.25);
    }
    let cover = 1.0 - smoothstep(-0.5, 0.5, d);
    let across = local.y / max(half_size.y, 1.0);
    let fade = clamp((1.0 - abs(across)) * 3.0, 0.0, 1.0);
    return cover * fade;
}

@fragment
fn fs_main(
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) @interpolate(flat, either) params: vec4<f32>,
) -> @location(0) vec4<f32> {
    let radius = params.x;
    // Positive: an outline that wide; negative: a glow that wide outside the edge.
    let edge = params.y;
    // The packed effect word: a fill's sheen strength or a glow's pulse depth, the
    // pattern's strength and its seed.
    let bits = u32(params.z + 0.5);
    let amount = f32(bits & 255u) / 255.0;
    let strength = f32((bits >> 8u) & 255u) / 255.0;
    let seed = f32((bits >> 16u) & 255u);
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
    if (strength > 0.0) {
        let light = pattern(local, half_size, strength, seed, phase) * min(strength * 2.0, 1.0);
        out = vec4<f32>(
            mix(out.rgb, vec3<f32>(1.0, 1.0, 1.0), light),
            out.a + (1.0 - out.a) * light * 0.5,
        );
    }
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
