// Textured, tinted sprite. Colour is straight alpha and multiplies the texel, as the
// plain (unmasked) path of osu-framework's sh_Texture2D.vs / sh_Texture.fs. Vertices
// come from the sprite batcher (`src/sprite.rs`, ADR-0020) with atlas UVs baked in;
// wrap modes and masking are not supported yet.

struct Globals {
    // Maps positions to clip space (y up, z in [0, 1]).
    projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(2) @binding(0) var sprite_sampler: sampler;

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

@fragment
fn fs_main(@location(0) colour: vec4<f32>, @location(1) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return colour * textureSample(sprite_texture, sprite_sampler, uv);
}
