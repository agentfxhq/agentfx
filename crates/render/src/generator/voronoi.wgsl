// Voronoi cells (voronoi.rs). gp(i): 0 re, 1 im, 2 zoom, 3 turn, 4 jitter, 5 flow, 6 mode,
// 7 thickness, 8 color-speed, 9 color-offset, 10 aa
fn vo_smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * fma(-2.0, t, 3.0);
}
fn vo_glow(d: f32, w: f32) -> f32 {
    let hw = fma(0.6, w, 1e-4);
    let k = hw / (d + hw);
    return fma(0.55, k * k, vo_smooth(w, 0.5 * w, d));
}
fn vo_wave(x: f32) -> f32 {
    let t = abs(fma(2.0, x - floor(x), -1.0));
    return t * t * fma(-2.0, t, 3.0);
}
fn vo_seed(cx: i32, cy: i32, s: u32, jit: f32, tt: f32) -> vec2<f32> {
    let a = unit(hash(cx, cy, s));
    let b = unit(hash(cx, cy, s + 1u));
    let r = fma(0.6, a, 0.7);
    let px = vo_wave(fma(r, tt, a));
    let py = vo_wave(fma(1.3, r * tt, b));
    return vec2<f32>(fma(jit, px - 0.5, 0.5), fma(jit, py - 0.5, 0.5));
}
fn sample(x: f32, y: f32) -> vec4<f32> {
    let s = pu(12u);
    let jit = gp(4u);
    let tt = pf(13u) * gp(5u);
    let xf = floor(x);
    let yf = floor(y);
    let nx = i32(xf);
    let ny = i32(yf);
    let fx = x - xf;
    let fy = y - yf;
    var md = 8.0;
    var m2 = 8.0;
    var mr = vec2<f32>(0.0);
    var mg = vec2<i32>(0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let o = vo_seed(nx + i, ny + j, s, jit, tt);
            let rx = f32(i) + o.x - fx;
            let ry = f32(j) + o.y - fy;
            let d = fma(rx, rx, ry * ry);
            if (d < md) {
                m2 = md;
                md = d;
                mr = vec2<f32>(rx, ry);
                mg = vec2<i32>(i, j);
            } else if (d < m2) {
                m2 = d;
            }
        }
    }
    var bd = 8.0;
    for (var j = -2; j <= 2; j++) {
        for (var i = -2; i <= 2; i++) {
            let gx = mg.x + i;
            let gy = mg.y + j;
            let o = vo_seed(nx + gx, ny + gy, s, jit, tt);
            let rx = f32(gx) + o.x - fx;
            let ry = f32(gy) + o.y - fy;
            let ex = rx - mr.x;
            let ey = ry - mr.y;
            let e2 = fma(ex, ex, ey * ey);
            if (e2 > 1e-5) {
                let l = sqrt(e2);
                let d = fma(0.5 * (mr.x + rx), ex / l, 0.5 * (mr.y + ry) * (ey / l));
                if (d < bd) { bd = d; }
            }
        }
    }
    let f1 = sqrt(md);
    let f2 = sqrt(m2);
    let speed = gp(8u);
    let offset = gp(9u);
    let w = gp(7u);
    let hid = unit(hash(nx + mg.x, ny + mg.y, s + 2u));
    let mode = i32(round_away(gp(6u)));
    let edge = vo_smooth(w, 0.5 * w, bd);
    var rgb = vec3<f32>(0.0);
    if (mode == 1) {
        let hw = fma(4.0, w, 1e-4);
        let q = hw / (bd + hw);
        let i = fma(0.6, q * q, vo_glow(bd, w));
        let p = palette(fma(speed, fma(0.04, x, 0.025 * y), offset));
        let fc = palette(fma(speed, hid, offset));
        let hot = 0.3 * max(i - 0.8, 0.0);
        rgb = min(fma(p.rgb, vec3<f32>(i), fma(vec3<f32>(0.12), fc.rgb, vec3<f32>(hot))), vec3<f32>(1.0));
    } else if (mode == 2) {
        let r = f2 - f1;
        let p = palette(fma(speed, fma(0.6, r, 0.5 * hid), offset));
        var l = fma(0.35, min(r, 1.0), fma(-0.65, edge, 0.65));
        l = fma(0.25, edge, l);
        rgb = min(p.rgb * l, vec3<f32>(1.0));
    } else {
        let p = palette(fma(speed, hid, offset));
        let l = fma(-0.9, min(f1, 1.0), 1.15) * fma(-0.85, edge, 1.0);
        rgb = min(p.rgb * l, vec3<f32>(1.0));
    }
    return vec4<f32>(rgb, 1.0);
}
