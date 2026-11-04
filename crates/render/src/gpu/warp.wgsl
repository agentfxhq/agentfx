// warp::apply (kaleido, tunnel) on the GPU: the same f32 expressions as warp.rs, every
// multiply-add an explicit fma, constants as exact f32 decimals.
// P: 0 w, 1 h, 2 kind, 3..6 dom x0 y0 x1 y1 (i32), 7 cx, 8 cy, 9 amount,
//    10 ang, 11 seg, 12 hseg, 13 invseg, 14 mirror, 15 invzoom,
//    16 rc, 17 rs, 18 f, 19 invf, 20 plane, 21 rho, 22 yc, 23 cc, 24 gap,
//    25 twist, 26 dz, 27 fog, 28..31 fog color (premultiplied 0..255), 32 k, 33 tcx, 34 ty1,
//    distort pack: 35 n (taps), 36 37 step, 38 39 noise origin, 40 inv, 41 42 drift,
//    43 octaves, 44 45 seeds, 46 disp, 48.. tap table (4 words per tap, see warp.rs)
// map: the displacement map (displace by a layer), else a dummy
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> src: array<u32>;
@group(0) @binding(3) var<storage, read_write> dmap: array<u32>;

fn tap(i: u32) -> vec4<f32> {
    let b = 48u + 4u * i;
    return vec4<f32>(pf(b), pf(b + 1u), pf(b + 2u), pf(b + 3u));
}

// generator::gnoise / fbm (f32, the same expressions as generator/common.wgsl)
fn wn_corner(ix: i32, iy: i32, seed: u32, dx: f32, dy: f32) -> f32 {
    let h = hash(ix, iy, seed);
    let gx = fma(f32(h & 0xffffu), 1.0 / 32768.0, -1.0);
    let gy = fma(f32(h >> 16u), 1.0 / 32768.0, -1.0);
    return fma(gx, dx, gy * dy);
}
fn wn_fade(t: f32) -> f32 { return t * t * t * fma(t, fma(t, 6.0, -15.0), 10.0); }
fn wnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xf = floor(x);
    let yf = floor(y);
    let ix = i32(xf);
    let iy = i32(yf);
    let tx = x - xf;
    let ty = y - yf;
    let sx = wn_fade(tx);
    let sy = wn_fade(ty);
    let a = wn_corner(ix, iy, seed, tx, ty);
    let b = wn_corner(ix + 1, iy, seed, tx - 1.0, ty);
    let c = wn_corner(ix, iy + 1, seed, tx, ty - 1.0);
    let d = wn_corner(ix + 1, iy + 1, seed, tx - 1.0, ty - 1.0);
    let ab = fma(b - a, sx, a);
    let cd = fma(d - c, sx, c);
    return fma(cd - ab, sy, ab);
}
fn wfbm(x0: f32, y0: f32, oct: u32, seed: u32) -> f32 {
    var x = x0;
    var y = y0;
    var sum = 0.0;
    var amp = 0.5;
    for (var o = 0u; o < oct; o++) {
        sum = fma(amp, wnoise(x, y, seed + o * 7919u), sum);
        let nx = fma(1.6, x, 1.2 * y);
        let ny = fma(-1.2, x, 1.6 * y);
        x = nx;
        y = ny;
        amp = amp * 0.5;
    }
    return sum * 1.6;
}

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

// (u, z, coverage); coverage 0 = miss
fn hit(ex: f32, ey: f32) -> vec3<f32> {
    var u = 0.0;
    var z = 0.0;
    if (pu(20u) == 1u) {
        if (ey <= 1.0000000116860974e-07) { return vec3<f32>(0.0); }
        let t = 1.0 / ey;
        u = t * ex;
        z = t;
    } else {
        let yc = pf(22u);
        let cc = pf(23u);
        let b = ey * yc;
        let d2 = fma(ex, ex, ey * ey);
        let disc = fma(b, b, -(d2 * cc));
        let sq = sqrt(disc);
        var t = 0.0;
        if (b >= 0.0) { t = (b + sq) / max(d2, 9.999999960041972e-13); } else { t = -cc / (sq - b); }
        let vx = t * ex;
        let vy = fma(t, ey, -yc);
        u = pf(21u) * atan2p(vx, vy);
        z = t;
    }
    let pxu = pf(18u) / z;
    let e = WPI - abs(u);
    let cov = max(clamp(fma(e, pxu, 0.5), 0.0, 1.0), fma(-pf(24u), pxu, 1.0));
    if (cov <= 0.0) { return vec3<f32>(0.0); }
    return vec3<f32>(u, z, cov);
}

fn ray_clamp(oy: f32, cy: f32) -> f32 {
    let by = (f32(pi(6u)) - 0.5) - cy;
    if (by > 0.0 && oy > by) { return by / oy; }
    return 1.0;
}

// fog by z, the distance along the tube from the camera (not the travel dz)
fn fade(z: f32, s: vec4<f32>) -> vec4<f32> {
    let fz = pf(27u) * z;
    let f = 1.0 / fma(fz, fz, 1.0);
    let fc = vec4<f32>(pf(28u), pf(29u), pf(30u), pf(31u));
    return fma(s - fc, vec4<f32>(f), fc);
}

// warp.rs `pixel` / `map_pixel` for the distort pack (kinds 3..8)
fn distort_px(kind: u32, x: u32, y: u32, fx: f32, fy: f32, cx: f32, cy: f32, qx: f32, qy: f32) -> vec4<f32> {
    let n = pu(35u);
    if (kind <= 5u) {
        var acc = vec4<f32>(0.0);
        for (var i = 0u; i < n; i++) {
            let t = tap(i);
            var sx = 0.0;
            var sy = 0.0;
            if (kind == 3u) {
                sx = fma(pf(36u), t.x, fx);
                sy = fma(pf(37u), t.x, fy);
            } else if (kind == 4u) {
                sx = fma(qx, t.x, cx);
                sy = fma(qy, t.x, cy);
            } else {
                sx = fma(t.x, qx, fma(-t.y, qy, cx));
                sy = fma(t.y, qx, fma(t.x, qy, cy));
            }
            acc = fma(sample(2u, sx, sy), vec4<f32>(t.z), acc);
        }
        return acc;
    }
    if (kind == 6u) {
        var col = vec3<f32>(0.0);
        var cov = vec3<f32>(0.0);
        for (var i = 0u; i < n; i++) {
            let t = tap(i);
            let v = sample(2u, fma(qx, t.x, cx), fma(qy, t.x, cy));
            col = fma(v.xyz, t.yzw, col);
            cov = fma(vec3<f32>(v.w), t.yzw, cov);
        }
        return vec4<f32>(col, max(max(cov.x, cov.y), cov.z));
    }
    let disp = pf(46u);
    if (kind == 7u) {
        let nx = fma(fx - pf(38u), pf(40u), pf(41u));
        let ny = fma(fy - pf(39u), pf(40u), pf(42u));
        let dx = wfbm(nx, ny, pu(43u), pu(44u));
        let dy = wfbm(nx, ny, pu(43u), pu(45u));
        return sample(2u, fma(dx, disp, fx), fma(dy, disp, fy));
    }
    let m = dmap[y * pu(0u) + x];
    let ma = f32(ch(m, 3u));
    let dx = fma(-0.5, ma, f32(ch(m, 0u)));
    let dy = fma(-0.5, ma, f32(ch(m, 1u)));
    return sample(2u, fma(dx, disp, fx), fma(dy, disp, fy));
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
    if (kind >= 3u) {
        return distort_px(kind, x, y, fx, fy, cx, cy, qx, qy);
    }
    let rc = pf(16u);
    let rsn = pf(17u);
    let dx = fma(rc, qx, rsn * qy);
    let dy = fma(rc, qy, -(rsn * qx));
    let invf = pf(19u);
    let h = hit(dx * invf, dy * invf);
    let twist = pf(25u);
    let dz = pf(26u);
    if (kind == 1u) {
        if (h.z <= 0.0) { return vec4<f32>(0.0); }
        let zz = h.y + dz;
        let ur = fma(twist, zz, h.x);
        let k = pf(32u);
        let s = sample(1u, fma(ur, k, pf(33u)), fma(-zz, k, pf(34u)));
        return fade(h.y, s) * h.z;
    }
    let miss = sample(2u, cx + dx, cy + dy);
    if (h.z <= 0.0) { return miss; }
    let zz = max(h.y + dz, 0.0010000000474974513);
    let ur = fma(twist, zz, h.x);
    let iz = pf(18u) / zz;
    let ox = ur * iz;
    let oy = iz;
    let k = ray_clamp(oy, cy);
    let s = sample(0u, fma(ox, k, cx), fma(oy, k, cy));
    let col = fade(h.y, s);
    return fma(col - miss, vec4<f32>(h.z), miss);
}

@compute @workgroup_size(16, 16)
fn warp(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let v = warp_px(id.x, id.y);
    px[id.y * w + id.x] = pack(to_u8(v.x + 0.5), to_u8(v.y + 0.5), to_u8(v.z + 0.5), to_u8(v.w + 0.5));
}
