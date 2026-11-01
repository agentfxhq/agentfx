// Power-n Mandelbulb (mandelbulb.rs). gp(i): 0 power, 1 yaw, 2 pitch, 3 distance, 4 zoom, 5 turn,
// 6 iter, 7 steps, 8 halo, 9 fog, 10 occlusion, 11 rim, 12 color-speed, 13 color-offset, 14 aa;
// uniforms (ray.wgsl) UK: power, UK + 1: iterations (rounded)
const NP: u32 = 15u;

fn de(p: vec3<f32>) -> vec3<f32> {
    let n = gu(UK);
    let iter = u32(gu(UK + 1u));
    var z = p;
    var dr = 1.0;
    var r = 0.0;
    var trap = 1e10;
    for (var i = 0u; i < iter; i++) {
        let r2 = dot3(z, z);
        r = root(r2);
        if (r > 2.0) { break; }
        trap = mn(trap, r2);
        let lr = lg2(r);
        let rp = ex2((n - 1.0) * lr);
        dr = fma(rp * n, dr, 1.0);
        let st = sincost(atan2t(root(fma(z.x, z.x, z.y * z.y)), z.z), n);
        let sp = sincost(atan2t(z.y, z.x), n);
        let zr = rp * r;
        let zs = zr * st.x;
        z = vec3<f32>(fma(zs, sp.y, p.x), fma(zs, sp.x, p.y), fma(zr, st.y, p.z));
    }
    return vec3<f32>(0.3465736 * lg2(r) * r, rcp(dr), trap);
}
