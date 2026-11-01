// Raymarched 3D kinds (ray.rs): f32 math from add / mul / fma and bit casts only (ray::M for f32,
// operation for operation), the orbit camera march and the shading. The kind's file defines
// `const NP: u32` (its parameter count) and `fn de(p: vec3<f32>) -> vec3<f32>` (distance as two
// factors x * y, orbit trap z).
// Uniforms gu(i) after the parameters: 0-2 eye, 3-5 forward, 6-8 right, 9-11 up, 12-14 light,
// 15 epsilon per unit ray length, 16 bounding radius, 17 steps, 18 halo, 19 fog, 20 occlusion,
// 21 rim, 22 color-speed, 23 color-offset, 24 on: the kind's own (UK).
const UK: u32 = 24u;

fn gu(i: u32) -> f32 { return gp(NP + i); }
fn gu3(i: u32) -> vec3<f32> { return vec3<f32>(gu(i), gu(i + 1u), gu(i + 2u)); }

fn mn(a: f32, b: f32) -> f32 { return select(b, a, a < b); }
fn mx(a: f32, b: f32) -> f32 { return select(b, a, a > b); }
fn sat(x: f32) -> f32 { return select(select(x, 1.0, x > 1.0), 0.0, x < 0.0); }

fn rcp(x: f32) -> f32 {
    var y = bitcast<f32>(0x7ef311c3u - bitcast<u32>(x));
    for (var k = 0; k < 3; k++) {
        let e = fma(-x, y, 1.0);
        y = fma(y, e, y);
    }
    return y;
}

fn rsqrt(x: f32) -> f32 {
    var y = bitcast<f32>(0x5f3759dfu - (bitcast<u32>(x) >> 1u));
    let h = 0.5 * x;
    for (var k = 0; k < 3; k++) {
        y = y * fma(-(h * y), y, 1.5);
    }
    return y;
}

fn root(x: f32) -> f32 { return x * rsqrt(x); }

fn lg2(x: f32) -> f32 {
    let b = bitcast<u32>(x);
    var e = i32((b >> 23u) & 0xffu) - 127;
    var m = bitcast<f32>((b & 0x7fffffu) | 0x3f800000u);
    if (m > 1.5) {
        m = m * 0.5;
        e += 1;
    }
    let s = (m - 1.0) * rcp(m + 1.0);
    let s2 = s * s;
    let pl = fma(s2, fma(s2, fma(s2, fma(s2, 0.11111111, 0.14285715), 0.2), 0.33333334), 1.0);
    return fma(s * pl, 2.88539, f32(e));
}

fn ex2(x0: f32) -> f32 {
    let x = select(select(x0, 126.0, x0 > 126.0), -126.0, x0 < -126.0);
    let n = floor(x + 0.5);
    let f = x - n;
    let p = fma(f, fma(f, fma(f, fma(f, fma(f, fma(f, fma(f, 1.5252734e-5, 1.540353e-4), 1.3333558e-3), 9.618129e-3), 5.550411e-2), 0.2402265), 0.6931472), 1.0);
    return p * bitcast<f32>(u32(i32(n) + 127) << 23u);
}

fn atan2t(y: f32, x: f32) -> f32 {
    let ax = abs(x);
    let ay = abs(y);
    let hi = mx(mx(ax, ay), 1e-30);
    let lo = mn(ax, ay);
    let a = lo * rcp(hi);
    let a2 = a * a;
    let p = fma(a2, fma(a2, fma(a2, fma(a2, fma(a2, -0.0117212, 0.05265332), -0.11643287), 0.19354346), -0.33262347), 0.99997726);
    let sw = ay > ax;
    var r = fma(a * p, select(0.15915494, -0.15915494, sw), select(0.0, 0.25, sw));
    if (x < 0.0) { r = 0.5 - r; }
    return select(r, -r, y < 0.0);
}

// (sin, cos) of a * n turns
fn sincost(a: f32, n: f32) -> vec2<f32> {
    let f = fma(a, n, -floor(fma(a, n, 0.5)));
    let q = floor(fma(f, 4.0, 0.5));
    let r = fma(q, -0.25, f) * 6.2831855;
    let r2 = r * r;
    let s = r * fma(r2, fma(r2, fma(r2, fma(r2, 2.755732e-6, -1.984127e-4), 8.333334e-3), -0.16666667), 1.0);
    let c = fma(r2, fma(r2, fma(r2, fma(r2, 2.4801588e-5, -1.3888889e-3), 4.1666668e-2), -0.5), 1.0);
    let qi = (i32(q) % 4 + 4) % 4;
    if (qi == 0) { return vec2<f32>(s, c); }
    if (qi == 1) { return vec2<f32>(c, -s); }
    if (qi == 2) { return vec2<f32>(-s, -c); }
    return vec2<f32>(-c, s);
}

fn dot3(a: vec3<f32>, b: vec3<f32>) -> f32 { return fma(a.x, b.x, fma(a.y, b.y, a.z * b.z)); }

// 3x3 row-major matrix at uniform i times v (ray::mul3)
fn mul3(i: u32, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        fma(gu(i), v.x, fma(gu(i + 1u), v.y, gu(i + 2u) * v.z)),
        fma(gu(i + 3u), v.x, fma(gu(i + 4u), v.y, gu(i + 5u) * v.z)),
        fma(gu(i + 6u), v.x, fma(gu(i + 7u), v.y, gu(i + 8u) * v.z)));
}

// ray::march
fn sample(su: f32, sv: f32) -> vec4<f32> {
    let eye = gu3(0u);
    let fw = gu3(3u);
    let rt = gu3(6u);
    let up = gu3(9u);
    let li = gu3(12u);
    let epsk = gu(15u);
    let bound = gu(16u);
    let steps = u32(gu(17u));
    let halo = gu(18u);
    let fog = gu(19u);
    let occl = gu(20u);
    let rim = gu(21u);
    let speed = gu(22u);
    let offset = gu(23u);
    let rd0 = fma(vec3<f32>(su), rt, fma(vec3<f32>(sv), up, fw));
    let rn = rsqrt(dot3(rd0, rd0));
    let rd = rd0 * rn;
    let h = fma(-rd0, vec3<f32>(rn), li);
    let b = dot3(eye, rd);
    let c = fma(-bound, bound, dot3(eye, eye));
    let disc = fma(b, b, -c);
    var col = vec3<f32>(0.0);
    if (disc > 0.0) {
        let rs = rsqrt(disc);
        let t0 = mx(fma(-disc, rs, -b), 0.0);
        let tmax = fma(disc, rs, -b);
        var t = t0;
        var i = 0u;
        var hit = false;
        var trap = 0.0;
        var near = 1e10;
        while (i < steps) {
            let p = fma(rd, vec3<f32>(t), eye);
            let dt = de(p);
            let d = dt.x * dt.y;
            if (d < fma(epsk, t, 1e-5)) {
                hit = true;
                trap = dt.z;
                break;
            }
            let q = d * rcp(t + 0.05);
            near = mn(near, q);
            t = fma(dt.x, dt.y, t);
            i += 1u;
            if (t > tmax) { break; }
        }
        let fi = f32(i) * rcp(f32(steps));
        let fa = ex2(-(fog * (t - t0)));
        if (hit) {
            let p = fma(rd, vec3<f32>(t), eye);
            let e = fma(epsk, t, 1e-5) * 1.5;
            var n = vec3<f32>(0.0);
            for (var k = 0; k < 4; k++) {
                var kv = vec3<f32>(1.0, 1.0, 1.0);
                if (k == 0) { kv = vec3<f32>(1.0, -1.0, -1.0); }
                if (k == 1) { kv = vec3<f32>(-1.0, -1.0, 1.0); }
                if (k == 2) { kv = vec3<f32>(-1.0, 1.0, -1.0); }
                let dk = de(fma(kv, vec3<f32>(e), p));
                let d = dk.x * dk.y;
                n = fma(kv, vec3<f32>(d), n);
            }
            n = n * rsqrt(mx(dot3(n, n), 1e-30));
            let diff = mx(dot3(n, li), 0.0);
            let f = 1.0 - mx(-dot3(n, rd), 0.0);
            let rimv = rim * f * f;
            var sp = mx(dot3(n, h) * rsqrt(mx(dot3(h, h), 1e-30)), 0.0);
            for (var k = 0; k < 5; k++) { sp = sp * sp; }
            var ao = sat(fma(-occl, fi, 1.0));
            ao = ao * ao;
            let tc = root(trap) * 2.0;
            let base = palette(fma(speed, tc, offset)).rgb;
            let rc = palette(fma(speed, tc, offset + 0.5)).rgb;
            let lit = fma(diff, 0.85, 0.15) * ao;
            let rk = rimv * fma(ao, 0.6, 0.4);
            col = fma(vec3<f32>(0.7), vec3<f32>(sp * ao), fma(base, vec3<f32>(lit), rc * rk)) * fa;
        }
        let nk = near * 40.0;
        let kk = rcp(fma(nk, nk, 1.0));
        var g = halo * kk * 0.8;
        var gs = kk * 0.5;
        if (hit) {
            g = halo * fi * 0.9;
            gs = root(trap) * 2.0;
        }
        let gc = palette(fma(speed, gs, offset + 0.25)).rgb;
        col = fma(gc, vec3<f32>(g), col);
    }
    return vec4<f32>(sat(col.x), sat(col.y), sat(col.z), 1.0);
}
