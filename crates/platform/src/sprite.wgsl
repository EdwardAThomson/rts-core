// One textured, tinted quad per sprite, positioned in screen pixels with the origin at the top left.

struct Screen {
    size: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) tint: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    out.pos = vec4<f32>(pos.x / screen.size.x * 2.0 - 1.0, 1.0 - pos.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    out.tint = tint;
    return out;
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * in.tint;
}
