// Slider body prepass (ADR-0041): writes, per pixel, how close it is to the path,
// 1 on the path and 0 one radius away, into an R16Float target with Max blending,
// as osu-framework's sh_PathPrepass.fs. The mesh (`src/slider.rs`) is static: each
// segment is clipped here to the snaking range, then its quad is grown by the
// radius around the clipped segment.

// Mirrors `BodyUniforms` in src/slider.rs; shared with slider_body.wgsl.
struct Body {
    // Maps path coordinates (osu! pixels) to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
    // x: path radius; y, z: snaking range of path length; w: unused.
    path: vec4<f32>,
    // Read by slider_body.wgsl.
    bounds: vec4<f32>,
    tint: vec4<f32>,
    gradient: vec4<f32>,
    texels: array<vec4<u32>, 32>,
}

@group(0) @binding(0) var<uniform> body: Body;

struct VertexInput {
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
    @location(2) distance: vec2<f32>,
    @location(3) corner: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) start: vec2<f32>,
    @location(2) @interpolate(flat) end: vec2<f32>,
}

// Mirrors `SAME_DISTANCE` in src/slider.rs.
const SAME_DISTANCE: f32 = 1e-4;

// Point at path length `d` on the segment, as `clip_segment` in src/slider.rs.
fn at(in: VertexInput, d: f32) -> vec2<f32> {
    let len = in.distance.y - in.distance.x;
    if len <= SAME_DISTANCE {
        return in.start;
    }
    let w = (clamp(d, in.distance.x, in.distance.y) - in.distance.x) / len;
    return in.start + (in.end - in.start) * w;
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let radius = body.path.x;
    let d0 = body.path.y;
    let d1 = body.path.z;
    if in.distance.x > d1 || in.distance.y < d0 {
        // Outside the range: every corner at one point off screen.
        out.position = vec4<f32>(-2.0, -2.0, 0.0, 1.0);
        out.local = vec2<f32>(0.0);
        out.start = vec2<f32>(0.0);
        out.end = vec2<f32>(0.0);
        return out;
    }
    let s = at(in, d0);
    let e = at(in, d1);
    let dir = e - s;
    let length_squared = dot(dir, dir);
    var along = vec2<f32>(1.0, 0.0);
    if length_squared > 0.0 {
        along = dir / sqrt(length_squared);
    }
    let across = vec2<f32>(-along.y, along.x);
    let base = mix(s - along * radius, e + along * radius, in.corner.x);
    let local = base + across * (radius * in.corner.y);
    out.position = body.projection * vec4<f32>(local, 0.0, 1.0);
    out.local = local;
    out.start = s;
    out.end = e;
    return out;
}

// Ported from osu-framework 2026.921.1: osu.Framework/Resources/Shaders/sh_CircularProgressUtils.h
fn dst_to_line(start: vec2<f32>, end: vec2<f32>, pixel: vec2<f32>) -> f32 {
    let dir = end - start;
    let line_length_squared = dir.x * dir.x + dir.y * dir.y;
    if line_length_squared < 0.000001 {
        return distance(pixel, start);
    }
    let dir2 = pixel - start;
    let t = clamp(dot(dir2, dir), 0.0, line_length_squared) / line_length_squared;
    return distance(pixel, start + dir * t);
}

// Ported from osu-framework 2026.921.1: osu.Framework/Resources/Shaders/sh_PathPrepass.fs
@fragment
fn fs_main(
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) start: vec2<f32>,
    @location(2) @interpolate(flat) end: vec2<f32>,
) -> @location(0) vec4<f32> {
    let value = clamp(1.0 - dst_to_line(start, end, local) / body.path.x, 0.0, 1.0);
    return vec4<f32>(value, 0.0, 0.0, 0.0);
}
