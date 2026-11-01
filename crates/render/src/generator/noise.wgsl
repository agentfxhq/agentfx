// Domain-warped fBm (noise.rs). gp(i): 0 re, 1 im, 2 zoom, 3 turn, 4 warp, 5 octaves, 6 flow,
// 7 color-speed, 8 color-offset, 9 aa
fn sample(x: f32, y: f32) -> vec4<f32> {
    let s = pu(12u);
    let oct = clamp(u32(round_away(gp(5u))), 1u, 8u);
    let w = gp(4u);
    let tt = pf(13u) * gp(6u);
    let qx = fbm(vec2<f32>(x + tt, y), oct, s);
    let qy = fbm(vec2<f32>(x + 5.2, fma(tt, -0.7, y + 1.3)), oct, s + 1u);
    let rx = fbm(vec2<f32>(fma(w, qx, x + 1.7), fma(w, qy, y + 9.2)), oct, s + 2u);
    let ry = fbm(vec2<f32>(fma(w, qx, x + 8.3), fma(w, qy, y + 2.8)), oct, s + 3u);
    let f = fbm(vec2<f32>(fma(w, rx, x), fma(w, ry, y)), oct, s + 4u);
    let c = palette(fma(gp(7u), fma(0.8, f, 0.8 * rx), gp(8u)));
    let l = clamp(fma(0.7, f, 0.5), 0.0, 1.0);
    let r = clamp(fma(-2.5, abs(ry), 1.0), 0.0, 1.0);
    let k = fma(0.6, r * r * r, fma(0.8 * l, l, 0.2));
    return vec4<f32>(min(c.rgb * k, vec3<f32>(1.0)), c.a);
}
