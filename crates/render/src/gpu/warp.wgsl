// warp::apply (kaleido) on the GPU: the same f32 expressions as warp.rs, every
// multiply-add an explicit fma, constants as exact f32 decimals.
// P: 0 w, 1 h, 2 kind, 3..6 dom x0 y0 x1 y1 (i32), 7 cx, 8 cy, 9 amount,
//    10 ang, 11 seg, 12 hseg, 13 invseg, 14 mirror, 15 invzoom
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> src: array<u32>;

const WPI: f32 = 3.1415927410125732;
const WPIO2: f32 = 1.5707963705062866;

fn atan2p(y: f32, x: f32) -> f32 {
    let ax = abs(x);
    let ay = abs(y);
    let mx = max(ax, ay);
    if (mx == 0.0) { return 0.0; }
    let a = min(ax, ay) / mx;
    let s = a * a;
    var q = fma(s, 0.0028662257827818394, -0.016165737062692642);
    q = fma(s, q, 0.042909614741802216);
    q = fma(s, q, -0.07528963685035706);
    q = fma(s, q, 0.10656263679265976);
    q = fma(s, q, -0.14208899438381195);
    q = fma(s, q, 0.19993551075458527);
    q = fma(s, q, -0.33333146572113037);
    var r = fma(a * s, q, a);
    if (ay > ax) { r = WPIO2 - r; }
    if (x < 0.0) { r = WPI - r; }
    if (y < 0.0) { r = -r; }
    return r;
}

fn sincosp(b: f32) -> vec2<f32> {
    let q = floor(fma(b, 0.6366197466850281, 0.5));
    let r = fma(-q, -4.371138828673793e-08, fma(-q, 1.5707963705062866, b));
    let s2 = r * r;
    let sn = fma(r * s2, fma(s2, fma(s2, -0.00019841270113829523, 0.008333333767950535), -0.1666666716337204), r);
    let cs = fma(s2, fma(s2, fma(s2, fma(s2, 2.4801587642286904e-05, -0.0013888889225199819), 0.0416666679084301), -0.5), 1.0);
    let k = i32(q) & 3;
    if (k == 0) { return vec2<f32>(sn, cs); }
    if (k == 1) { return vec2<f32>(cs, -sn); }
    if (k == 2) { return vec2<f32>(-sn, -cs); }
    return vec2<f32>(-cs, sn);
}

fn reme(i: i32, n: i32) -> i32 {
    let m = i % n;
    return select(m, m + n, m < 0);
}

fn texel(mode: u32, ix: i32, iy: i32) -> vec4<f32> {
    let x0 = pi(3u);
    let y0 = pi(4u);
    let x1 = pi(5u);
    let y1 = pi(6u);
    let nx = x1 - x0;
    let ny = y1 - y0;
    var x = ix;
    var y = iy;
    if (mode == 0u) {
        var mx = reme(ix - x0, 2 * nx);
        if (mx >= nx) { mx = 2 * nx - 1 - mx; }
        var my = reme(iy - y0, 2 * ny);
        if (my >= ny) { my = 2 * ny - 1 - my; }
        x = x0 + mx;
        y = y0 + my;
    } else if (mode == 1u) {
        x = x0 + reme(ix - x0, nx);
        y = y0 + reme(iy - y0, ny);
    } else if (ix < x0 || iy < y0 || ix >= x1 || iy >= y1) {
        return vec4<f32>(0.0);
    }
    let p = src[u32(y) * pu(0u) + u32(x)];
    return vec4<f32>(f32(ch(p, 0u)), f32(ch(p, 1u)), f32(ch(p, 2u)), f32(ch(p, 3u)));
}

fn sample(mode: u32, x: f32, y: f32) -> vec4<f32> {
    let fx = clamp(x, -1e7, 1e7) - 0.5;
    let fy = clamp(y, -1e7, 1e7) - 0.5;
    let xf = floor(fx);
    let yf = floor(fy);
    let tx = fx - xf;
    let ty = fy - yf;
    let ix = i32(xf);
    let iy = i32(yf);
    let a = texel(mode, ix, iy);
    let b = texel(mode, ix + 1, iy);
    let c = texel(mode, ix, iy + 1);
    let d = texel(mode, ix + 1, iy + 1);
    let top = fma(b - a, vec4<f32>(tx), a);
    let bot = fma(d - c, vec4<f32>(tx), c);
    return fma(bot - top, vec4<f32>(ty), top);
}

fn warp_px(x: u32, y: u32) -> vec4<f32> {
    let cx = pf(7u);
    let cy = pf(8u);
    let fx = f32(x) + 0.5;
    let fy = f32(y) + 0.5;
    let qx = fx - cx;
    let qy = fy - cy;
    let kind = pu(2u);
    if (kind == 0u) {
        let r = sqrt(fma(qx, qx, qy * qy));
        let ang = pf(10u);
        let seg = pf(11u);
        let hseg = pf(12u);
        let t1 = atan2p(qy, qx) - ang;
        let a0 = fma(-seg, floor(t1 * pf(13u)), t1);
        var a1 = a0;
        if (pu(14u) == 1u) { a1 = hseg - abs(a0 - hseg); }
        let sc = sincosp(a1 + ang);
        let rs = r * pf(15u);
        let sx = fma(rs, sc.y, cx);
        let sy = fma(rs, sc.x, cy);
        let am = pf(9u);
        return sample(0u, fma(am, sx - fx, fx), fma(am, sy - fy, fy));
    }
    return vec4<f32>(0.0);
}

@compute @workgroup_size(16, 16)
fn warp(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let v = warp_px(id.x, id.y);
    px[id.y * w + id.x] = pack(to_u8(v.x + 0.5), to_u8(v.y + 0.5), to_u8(v.z + 0.5), to_u8(v.w + 0.5));
}
