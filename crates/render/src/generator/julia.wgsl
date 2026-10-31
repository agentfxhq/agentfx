// Julia set (julia.rs). gp(i): 0 cx, 1 cy, 2 re, 3 im, 4 zoom, 5 depth, 6 turn, 7 iter,
// 8 bailout, 9 color-speed, 10 color-offset, 11 aa
// (u, v): the point of the complex plane (the view is composed into the pixel mapping)
fn sample(u: f32, v: f32) -> vec4<f32> {
    let b = gp(8u);
    let e = escape(vec2<f32>(u, v), vec2<f32>(gp(0u), gp(1u)), u32(round_away(gp(7u))), b * b);
    return fractal_color(e, gp(9u), gp(10u));
}
