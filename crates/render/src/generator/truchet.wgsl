// Truchet tiles (truchet.rs). gp(i): 0 re, 1 im, 2 zoom, 3 turn, 4 thickness, 5 phase, 6 mode,
// 7 color-speed, 8 color-offset, 9 aa
fn tr_smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * fma(-2.0, t, 3.0);
}
fn tr_glow(d: f32, w: f32) -> f32 {
    let hw = fma(0.6, w, 1e-4);
    let k = hw / (d + hw);
    return fma(0.55, k * k, tr_smooth(w, 0.5 * w, d));
}
fn sample(x: f32, y: f32) -> vec4<f32> {
    let xf = floor(x);
    let yf = floor(y);
    let cx = i32(xf);
    let cy = i32(yf);
    let s = pu(12u);
    let h = hash(cx, cy, s);
    let delay = unit(hash(cx, cy, s + 1u)) * 0.6;
    let q = gp(5u) - delay;
    let turns = floor(q);
    let fr = clamp((q - turns) / 0.4, 0.0, 1.0);
    let e = fr * fr * fma(-2.0, fr, 3.0);
    let flip = (h ^ bitcast<u32>(i32(turns))) & 1u;
    let sv = e * 0.41421356;
    let s2 = sv * sv;
    let den = 1.0 + s2;
    let c = (1.0 - s2) / den;
    let sn = (sv + sv) / den;
    let ax = x - xf - 0.5;
    let ay = y - yf - 0.5;
    let rx = fma(c, ax, sn * ay);
    let ry = fma(c, ay, -(sn * ax));
    var k = 0.5;
    if (flip != 0u) { k = -0.5; }
    let w = gp(4u);
    let mode = i32(round_away(gp(6u)));
    let d1x = rx + k;
    let d1y = ry + 0.5;
    let d2x = rx - k;
    let d2y = ry - 0.5;
    let l1 = sqrt(fma(d1x, d1x, d1y * d1y));
    let l2 = sqrt(fma(d2x, d2x, d2y * d2y));
    var l = l2;
    if (l1 < l2) { l = l1; }
    var sum = 0.0;
    if (mode >= 1) {
        sum = tr_glow(abs(l - 1.0 / 3.0), w) + tr_glow(abs(l - 2.0 / 3.0), w);
    } else {
        sum = tr_glow(abs(l - 0.5), w);
    }
    let pos = fma(gp(7u), fma(0.045, x, 0.03 * y), gp(8u));
    let col = palette(pos);
    let b = palette(pos + 0.5);
    let hot = 0.3 * max(sum - 0.8, 0.0);
    let rgb = min(fma(col.rgb, vec3<f32>(sum), fma(vec3<f32>(0.07), b.rgb, vec3<f32>(hot))), vec3<f32>(1.0));
    return vec4<f32>(rgb, 1.0);
}
