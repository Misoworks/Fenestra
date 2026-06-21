struct Globals {
    viewport: vec2<f32>,
    _padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> globals: Globals;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) rect_origin: vec2<f32>,
    @location(2) rect_size: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) radius: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radius: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let ndc = vec2<f32>(
        (input.position.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (input.position.y / globals.viewport.y) * 2.0,
    );

    output.position = vec4<f32>(ndc, 0.0, 1.0);
    output.local = input.position - input.rect_origin;
    output.size = input.rect_size;
    output.color = input.color;
    output.radius = input.radius;
    return output;
}

fn rounded_rect_alpha(local: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    if (radius <= 0.0) {
        return 1.0;
    }

    let clamped_radius = min(radius, min(size.x, size.y) * 0.5);
    let centered = local - size * 0.5;
    let corner = abs(centered) - size * 0.5 + vec2<f32>(clamped_radius);
    let distance = length(max(corner, vec2<f32>(0.0))) + min(max(corner.x, corner.y), 0.0) - clamped_radius;
    return 1.0 - smoothstep(0.0, 1.0, distance);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = rounded_rect_alpha(input.local, input.size, input.radius);
    return vec4<f32>(input.color.rgb, input.color.a * alpha);
}
