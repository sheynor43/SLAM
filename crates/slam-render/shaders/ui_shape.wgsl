// Interface shape (ADR-0116, ADR-0123): a rounded rectangle, filled or outlined, optionally
// skewed, tinting a texture region, with a band of light sweeping across it and sparse
// outlined triangles floating up it (ADR-0131, ADR-0136); or the soft, pulsing glow around such a
// rectangle. A star and the menu pointer take the same edge, outline and glow (ADR-0136). The skew is in the
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

// Signed distance to the outline of an upward-pointing equilateral triangle whose corners
// are `r` from its centre (y down): negative inside.
fn triangle(p: vec2<f32>, r: f32) -> f32 {
    let base = p.y - r * 0.5;
    let side = abs(p.x) * 0.8660254 - p.y * 0.5 - r * 0.5;
    return max(base, side);
}

// Signed distance to a five-pointed star, point up, its points `r` from the centre and its
// inner corners `r * rf` (after Inigo Quilez's `sdStar5`, MIT).
fn star5(p_in: vec2<f32>, r: f32, rf: f32) -> f32 {
    let k1 = vec2<f32>(0.809016994375, -0.587785252292);
    let k2 = vec2<f32>(-k1.x, k1.y);
    var p = vec2<f32>(abs(p_in.x), -p_in.y);
    p = p - 2.0 * max(dot(k1, p), 0.0) * k1;
    p = p - 2.0 * max(dot(k2, p), 0.0) * k2;
    p.x = abs(p.x);
    p.y = p.y - r;
    let ba = rf * vec2<f32>(-k1.y, k1.x) - vec2<f32>(0.0, 1.0);
    let h = clamp(dot(p, ba) / dot(ba, ba), 0.0, r);
    return length(p - ba * h) * sign(p.y * ba.x - p.x * ba.y);
}

// Signed distance to the menu pointer filling a box of half size `hs`: an arrowhead with its
// tip at the top centre, its wings at the bottom corners and a notch between them.
fn arrow(p: vec2<f32>, hs: vec2<f32>) -> f32 {
    var v = array<vec2<f32>, 4>(
        vec2<f32>(0.0, -hs.y),
        vec2<f32>(hs.x, hs.y),
        vec2<f32>(0.0, hs.y * 0.42),
        vec2<f32>(-hs.x, hs.y),
    );
    var d = dot(p - v[0], p - v[0]);
    var s = 1.0;
    var j = 3u;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let e = v[j] - v[i];
        let w = p - v[i];
        let b = w - e * clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
        d = min(d, dot(b, b));
        let c0 = p.y >= v[i].y;
        let c1 = v[j].y > p.y;
        let c2 = e.x * w.y > e.y * w.x;
        if ((c0 && c1 && c2) || (!c0 && !c1 && !c2)) {
            s = -s;
        }
        j = i;
    }
    return s * sqrt(d);
}

// Coverage in [0, 1] of the outlines of triangles floating up the shape under `local`
// (pixels from its centre), after lazer's `TrianglesV2`: columns a cell wide, each rising at
// its own speed, a cell holding at most one triangle of hashed size and place; neighbouring
// cells are looked at too, so a triangle may reach past its own. `strength` sets how many
// there are and how fast they rise; `seed` differs per shape.
fn pattern(local: vec2<f32>, half_size: vec2<f32>, strength: f32, seed: f32, phase: f32) -> f32 {
    // Capped, so a huge panel does not get huge triangles.
    let cell = clamp(half_size.y * 1.6, 20.0, 120.0);
    let px = (local.x + half_size.x) / cell;
    var cover = 0.0;
    for (var dx = -1; dx <= 1; dx = dx + 1) {
        let col = floor(px) + f32(dx);
        // The column's speed: around half the base, never below a quarter (lazer's
        // normal spread). A multiple of 1/16: the drift wraps every 1024 cycles
        // (`set_drift`), and 1024 times the rate is then a whole number of cells.
        let spread = hash(vec3<f32>(col + seed * 13.0, 7.0, 5.0));
        let rate = floor((0.6 + 2.4 * strength) * (0.4 + 0.9 * spread) * 16.0 + 0.5) / 16.0;
        let py = (local.y + half_size.y) / cell + phase * rate;
        for (var dy = -1; dy <= 1; dy = dy + 1) {
            let row = floor(py) + f32(dy);
            let key = vec3<f32>(col + seed * 13.0, row, 0.0);
            if (hash(key) > 0.35 + 0.35 * strength) {
                continue;
            }
            let r = (0.45 + 0.55 * hash(key + vec3<f32>(0.0, 0.0, 3.0))) * cell * 0.75;
            let centre = vec2<f32>(
                col + hash(key + vec3<f32>(0.0, 0.0, 1.0)),
                row + hash(key + vec3<f32>(0.0, 0.0, 2.0)),
            );
            let p = (vec2<f32>(px, py) - centre) * cell;
            let d = triangle(p, r);
            let width = max(1.0, r * 0.03);
            cover = max(cover, 1.0 - smoothstep(width * 0.5, width * 0.5 + 1.0, abs(d)));
        }
    }
    return cover;
}

// Signed distance to the shape of `kind`: 0 and 1 a rounded rectangle, 2 a star, 3 the
// pointer.
fn shape_distance(kind: u32, local: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    if (kind == 2u) {
        let r = min(half_size.x, half_size.y);
        return star5(local, r, 0.45) - radius;
    }
    if (kind == 3u) {
        return arrow(local, half_size - vec2<f32>(radius, radius)) - radius;
    }
    return rounded_box(local, half_size, radius);
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
    let seed = f32((bits >> 16u) & 63u);
    // 0 a rectangle, 1 a rectangle drawing only its pattern in its colour (a layer over
    // another shape), 2 a star, 3 the pointer.
    let kind = (bits >> 22u) & 3u;
    // Cycles of the sheen's sweep or the glow's pulse.
    let phase = params.w;
    let d = shape_distance(kind, local, half_size, radius);
    let aa = max(fwidth(d), 1e-4);
    // Gradients write corner colours a margin past the shape, which may step out of range.
    let tint = clamp(colour, vec4<f32>(0.0), vec4<f32>(1.0));
    let texel = tint * textureSample(shape_texture, shape_sampler, uv);
    if (edge < 0.0) {
        let width = -edge;
        let outside = clamp(0.5 + d / aa, 0.0, 1.0);
        let fall = 1.0 - clamp(d / width, 0.0, 1.0);
        let pulse = 1.0 - amount * 0.5 * (1.0 + sin(6.2831853 * phase));
        // The colour alone: the quad reaches past the texture's region by the glow, where
        // the atlas holds other images or nothing.
        return vec4<f32>(tint.rgb, tint.a * outside * fall * fall * pulse);
    }
    var coverage = clamp(0.5 - d / aa, 0.0, 1.0);
    if (edge > 0.0) {
        coverage = coverage * clamp(0.5 + (d + edge) / aa, 0.0, 1.0);
    }
    if (kind == 1u) {
        // The pattern alone, in the shape's colour.
        if (strength <= 0.0) {
            return vec4<f32>(0.0);
        }
        let lines = pattern(local, half_size, strength, seed, phase);
        return vec4<f32>(texel.rgb, texel.a * lines * coverage);
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
