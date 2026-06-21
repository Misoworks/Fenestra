struct Globals {
    viewport: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

@group(1) @binding(0)
var image_sampler: sampler;

@group(1) @binding(1)
var image_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let ndc = vec2<f32>(
        (input.position.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (input.position.y / globals.viewport.y) * 2.0,
    );
    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(image_texture, image_sampler, input.uv);
}