// Kaleidoscopic IFS (kifs.rs). gp(i): 0 mode, 1 fold-scale, 2 twist, 3 tilt, 4-6 cx cy cz,
// 7 yaw, 8 pitch, 9 distance, 10 zoom, 11 turn, 12 iter, 13 steps, 14 halo, 15 fog,
// 16 occlusion, 17 rim, 18 color-speed, 19 color-offset, 20 aa; uniforms (ray.wgsl) from UK:
// 0-8 twist matrix, 9-17 tilt matrix, 18 scale, 19-21 c (scale - 1), 22 iterations, 23 mode
const NP: u32 = 21u;

// reflection in x = -y: (a, b) -> (-b, -a) when a + b < 0
fn kflip(a: f32, b: f32) -> vec2<f32> { return select(vec2<f32>(a, b), vec2<f32>(-b, -a), a + b < 0.0); }
// reflection in x = y: (a, b) -> (b, a) when a < b
fn ksort(a: f32, b: f32) -> vec2<f32> { return select(vec2<f32>(a, b), vec2<f32>(b, a), a < b); }

fn de(p: vec3<f32>) -> vec3<f32> {
    let sc = gu(UK + 18u);
    let k = gu3(UK + 19u);
    let iter = u32(gu(UK + 22u));
    let mode = u32(gu(UK + 23u));
    var z = p;
    var s = 1.0;
    var trap = 1e10;
    for (var i = 0u; i < iter; i++) {
        z = mul3(UK, z);
        var x = z.x;
        var y = z.y;
        var w = z.z;
        var q: vec2<f32>;
        if (mode == 1u) {
            q = kflip(x, y); x = q.x; y = q.y;
            q = kflip(x, w); x = q.x; w = q.y;
            q = kflip(y, w); y = q.x; w = q.y;
        } else if (mode == 2u) {
            q = kflip(x, y); x = q.x; y = q.y;
            q = kflip(x, w); x = q.x; w = q.y;
            q = ksort(x, y); x = q.x; y = q.y;
            q = ksort(x, w); x = q.x; w = q.y;
        } else {
            x = abs(x);
            y = abs(y);
            w = abs(w);
            q = ksort(x, y); x = q.x; y = q.y;
            q = ksort(x, w); x = q.x; w = q.y;
            q = ksort(y, w); y = q.x; w = q.y;
        }
        z = mul3(UK + 9u, vec3<f32>(x, y, w));
        if (mode == 0u) {
            let zz = sc * z.z;
            z = vec3<f32>(fma(sc, z.x, -k.x), fma(sc, z.y, -k.y), select(zz, fma(sc, z.z, -k.z), zz > 0.5 * k.z));
        } else {
            z = fma(vec3<f32>(sc), z, -k);
        }
        s = s * sc;
        let r2 = dot3(z, z);
        if (i < 3u) { trap = mn(trap, r2); }
        if (r2 > 1e4) { break; }
    }
    let r2 = dot3(z, z);
    var d = fma(r2, rsqrt(r2), -1.0);
    if (mode == 0u) { d = mx(mx(abs(z.x), abs(z.y)), abs(z.z)) - 1.0; }
    return vec3<f32>(d, rcp(s), trap * 0.15);
}
