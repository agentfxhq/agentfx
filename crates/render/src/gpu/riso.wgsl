// look::riso on the GPU. Pass 1 (riso_sep): ink separation per pixel into density
// planes; pass 2 (riso_comp): misregistered plate sampling, density noise (coarse fbm
// grid from the CPU), paper-tooth speckle, Beer-Lambert recombination.
// P: 0 w, 1 h, 2 n, 3 has_paper, 4..6 paper rgb, 7 grain, 8 dens, 9 s, 10 l0x, 11 l0y,
//    12 i0, 13 j0, 14 gw, 15 gh, 16 seed, 17..28 dd[4][3], 29..44 g[4][4], 45..52 offs[4][2]
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> planes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> alpha: array<f32>;
@group(0) @binding(4) var<storage, read_write> dgrid: array<f32>;

fn dd(k: u32, c: u32) -> f32 { return pf(17u + k * 3u + c); }
fn gm(k: u32, j: u32) -> f32 { return pf(29u + k * 4u + j); }
fn paper() -> vec3<f32> { return vec3<f32>(pf(4u), pf(5u), pf(6u)); }

fn separate(c: vec3<f32>, n: u32) -> vec4<f32> {
    let t = vec3<f32>(-log(max(c.x, 0.02)), -log(max(c.y, 0.02)), -log(max(c.z, 0.02)));
    var b = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    for (var k = 0u; k < n; k++) {
        b[k] = dd(k, 0u) * t.x + dd(k, 1u) * t.y + dd(k, 2u) * t.z;
    }
    var o = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    for (var it = 0; it < 8; it++) {
        for (var k = 0u; k < n; k++) {
            var r = b[k];
            for (var j = 0u; j < n; j++) {
                if (j != k) { r -= gm(k, j) * o[j]; }
            }
            o[k] = clamp(r / max(gm(k, k), 1e-6), 0.0, 1.0);
        }
    }
    return vec4<f32>(o[0], o[1], o[2], o[3]);
}

@compute @workgroup_size(16, 16)
fn riso_sep(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let st = unpremul(px[i]);
    alpha[i] = st.a;
    if (st.a <= 0.0) {
        planes[i] = vec4<f32>(0.0);
        return;
    }
    let has_paper = pu(3u) != 0u;
    var rel = st.c;
    if (has_paper) {
        let pp = paper();
        rel = vec3<f32>(min(st.c.x / max(pp.x, 0.02), 1.0), min(st.c.y / max(pp.y, 0.02), 1.0), min(st.c.z / max(pp.z, 0.02), 1.0));
    }
    let d = separate(rel, pu(2u));
    planes[i] = d * select(st.a, 1.0, has_paper);
}

fn plane_at(x: i32, y: i32, k: u32) -> f32 {
    let w = i32(pu(0u));
    let h = i32(pu(1u));
    if (x < 0 || y < 0 || x >= w || y >= h) { return 0.0; }
    return planes[u32(y * w + x)][k];
}

fn grid_at(base: u32, i: u32, j: u32) -> f32 {
    let gw = pu(14u);
    let gh = pu(15u);
    return dgrid[base + min(j, gh - 1u) * gw + min(i, gw - 1u)];
}

@compute @workgroup_size(16, 16)
fn riso_comp(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let n = pu(2u);
    let has_paper = pu(3u) != 0u;
    let grain = pf(7u);
    let dens = pf(8u);
    let s = pf(9u);
    let l0x = pi(10u);
    let l0y = pi(11u);
    let i0 = pi(12u);
    let j0 = pi(13u);
    let seed = pu(16u);
    let ly_i = i32(id.y) + l0y;
    let gv = f32(ly_i) / 8.0 - f32(j0);
    let gj = u32(floor(gv));
    let ty = gv - floor(gv);
    let lx = f32(i32(id.x) + l0x);
    let ly = f32(ly_i);
    var od = vec3<f32>(0.0);
    var hit = false;
    for (var k = 0u; k < n; k++) {
        let dx = pf(45u + k * 2u);
        let dy = pf(46u + k * 2u);
        let fx = f32(id.x) - dx;
        let fy = f32(id.y) - dy;
        let x0 = floor(fx);
        let y0 = floor(fy);
        let tx = fx - x0;
        let tyy = fy - y0;
        let ix = i32(x0);
        let iy = i32(y0);
        let a0 = plane_at(ix, iy, k);
        let b0 = plane_at(ix + 1, iy, k);
        let c0 = plane_at(ix, iy + 1, k);
        let d0 = plane_at(ix + 1, iy + 1, k);
        var d = (a0 + (b0 - a0) * tx) + ((c0 + (d0 - c0) * tx) - (a0 + (b0 - a0) * tx)) * tyy;
        if (d <= 0.0) { continue; }
        hit = true;
        let ps = seed + k * 1013u;
        if (dens > 0.0) {
            let gu = lx / 8.0 - f32(i0);
            let gi = u32(floor(gu));
            let tx2 = gu - floor(gu);
            let base = k * pu(14u) * pu(15u);
            let top = grid_at(base, gi, gj) + (grid_at(base, gi + 1u, gj) - grid_at(base, gi, gj)) * tx2;
            let bot = grid_at(base, gi, gj + 1u) + (grid_at(base, gi + 1u, gj + 1u) - grid_at(base, gi, gj + 1u)) * tx2;
            d *= 1.0 + dens * 2.0 * (top + (bot - top) * ty);
        }
        if (grain > 0.0) {
            let qx = lx - round_away(dx);
            let qy = ly - round_away(dy);
            let f = value_noise(qx / (1.1 * s), qy / (1.1 * s), ps ^ 0x5151u);
            let f2 = unit(hash(i32(qx), i32(qy), ps ^ 0x7777u));
            d *= 1.0 - grain * (0.65 * smoothstep_(0.35, 0.95, f) + 0.35 * f2);
        }
        let dc = clamp(d, 0.0, 1.0);
        od += dc * vec3<f32>(dd(k, 0u), dd(k, 1u), dd(k, 2u));
    }
    let c = vec3<f32>(exp(-od.x), exp(-od.y), exp(-od.z));
    if (has_paper) {
        let a = alpha[i];
        if (a > 0.0) { px[i] = put(c, a, true, paper()); }
    } else {
        if (hit) { px[i] = put(c, 1.0, false, vec3<f32>(0.0)); } else { px[i] = 0u; }
    }
}
