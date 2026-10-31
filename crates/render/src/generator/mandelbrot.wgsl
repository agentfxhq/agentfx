// Mandelbrot set (mandelbrot.rs). gp(i): 0 re, 1 im, 2 zoom, 3 depth, 4 turn, 5 iter,
// 6 bailout, 7 color-speed, 8 color-offset, 9 aa
// (u, v): the point of the complex plane (the view is composed into the pixel mapping)
fn sample(u: f32, v: f32) -> vec4<f32> {
    let b = gp(6u);
    let e = escape(vec2<f32>(0.0, 0.0), vec2<f32>(u, v), u32(round_away(gp(5u))), b * b);
    return fractal_color(e, gp(7u), gp(8u));
}
