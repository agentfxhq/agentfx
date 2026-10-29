// light::shadow's signed distance field on the GPU (E4 phase 2): light::alpha_plane ->
// light::sdf (Euclidean distance transform inside and outside, exact up to the distance
// the shadow can see, see sh_cols) -> the field. Squared distances are integers and equal
// the CPU's; the final sqrt is the GPU's (may differ from IEEE by an ulp).
//
// Then either light.wgsl `shadow` composites on the resident frame, or `sh_occ` writes
// the occlusion (u16 pairs) for a CPU composite (the frame stays on the host and only
// its alpha goes up: 1 byte per pixel up, 2 down).
//
// P: 0 w, 1 h, 2 sw, 3 sh, 4 ds, 5 lx, 6 ly, 7 k, 8 rr, 9 inv_ds, 10 strength,
//    11..13 colour (light.wgsl), 14 src is packed alpha (1) or RGBA (0), 15 window C
@group(0) @binding(1) var<storage, read_write> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> cov: array<f32>;
@group(0) @binding(3) var<storage, read_write> gi: array<f32>;
@group(0) @binding(4) var<storage, read_write> go: array<f32>;
@group(0) @binding(5) var<storage, read_write> scr: array<u32>; // (unused)
@group(0) @binding(6) var<storage, read_write> fld: array<f32>;
@group(0) @binding(7) var<storage, read_write> occ: array<u32>;

const INF: f32 = 1e20;

fn alpha_at(i: u32) -> u32 {
    if (pu(14u) != 0u) { return (src[i >> 2u] >> (8u * (i & 3u))) & 255u; }
    return src[i] >> 24u;
}

// cov = alpha averaged over ds x ds blocks
@compute @workgroup_size(16, 16)
fn sh_cov(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    if (id.x >= sw || id.y >= pu(3u)) { return; }
    let w = pu(0u);
    let h = pu(1u);
    let ds = pu(4u);
    var sum = 0u;
    for (var y = id.y * ds; y < min((id.y + 1u) * ds, h); y++) {
        for (var x = id.x * ds; x < min((id.x + 1u) * ds, w); x++) {
            sum += alpha_at(y * w + x);
        }
    }
    // (GPU division may be off by an ulp; exactly half must stay >= 0.5: the inside test)
    let k = 255u * ds * ds;
    cov[id.y * sw + id.x] = select(f32(sum) / f32(k), 0.5, 2u * sum == k);
}

fn inside(i: u32) -> bool { return cov[i] >= 0.5; }

// Windowed exact EDT. The shadow only reads the field below the light radius (beyond it,
// no shadow; deep inside, full shadow), and the field is 1-Lipschitz, so distances up to
// C = ceil(rr / ds) + 3 field px (P[15]) are all that bilinear samples can turn into a
// different shadow. Within C the transform is exact (every feature at distance <= C lies
// in the (2C+1)^2 window); farther values come out >= C (or INF). Two separable passes,
// one invocation per pixel, O(C) each.

// columns: gi = squared distance to the nearest inside pixel in the column window,
// go = to the nearest outside pixel
@compute @workgroup_size(16, 16)
fn sh_cols(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    let sh = pu(3u);
    if (id.x >= sw || id.y >= sh) { return; }
    let c = i32(pu(15u));
    var bi = INF;
    var bo = INF;
    let y = i32(id.y);
    for (var dy = -c; dy <= c; dy++) {
        let yy = y + dy;
        if (yy < 0 || yy >= i32(sh)) { continue; }
        let d = f32(dy * dy);
        if (inside(u32(yy) * sw + id.x)) { bi = min(bi, d); } else { bo = min(bo, d); }
    }
    let i = id.y * sw + id.x;
    gi[i] = bi;
    go[i] = bo;
}

// rows: d^2 = min over the window of dx^2 + column distance (exact integers in f32)
@compute @workgroup_size(16, 16)
fn sh_rows(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    if (id.x >= sw || id.y >= pu(3u)) { return; }
    let c = i32(pu(15u));
    let base = id.y * sw;
    var bi = INF;
    var bo = INF;
    let x = i32(id.x);
    for (var dx = -c; dx <= c; dx++) {
        let xx = x + dx;
        if (xx < 0 || xx >= i32(sw)) { continue; }
        let d = f32(dx * dx);
        bi = min(bi, d + gi[base + u32(xx)]);
        bo = min(bo, d + go[base + u32(xx)]);
    }
    fld[base + id.x] = bi;
    occ[base + id.x] = bitcast<u32>(bo);
}

// light::sdf's combination (fld holds d_in, occ d_out)
@compute @workgroup_size(16, 16)
fn sh_field(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    if (id.x >= sw || id.y >= pu(3u)) { return; }
    let i = id.y * sw + id.x;
    let a = cov[i];
    var f: f32;
    if (a > 0.02 && a < 0.98) {
        f = 0.5 - a;
    } else if (a >= 0.5) {
        f = -(sqrt(bitcast<f32>(occ[i])) - 0.5);
    } else {
        f = sqrt(fld[i]) - 0.5;
    }
    fld[i] = f;
}

fn sample_fld(x: f32, y: f32) -> f32 {
    let w = pu(2u);
    let h = pu(3u);
    let fx = x - 0.5;
    let fy = y - 0.5;
    if (fx < 0.0 || fy < 0.0 || fx > f32(w - 1u) || fy > f32(h - 1u)) { return 1e6; }
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = u32(x0);
    let iy = u32(y0);
    let ix1 = min(ix + 1u, w - 1u);
    let iy1 = min(iy + 1u, h - 1u);
    let a = fld[iy * w + ix];
    let b = fld[iy * w + ix1];
    let c = fld[iy1 * w + ix];
    let d = fld[iy1 * w + ix1];
    return (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty;
}

fn occ_at(i: u32) -> f32 {
    let w = pu(0u);
    let x = f32(i % w) + 0.5;
    let y = f32(i / w) + 0.5;
    let lx = pf(5u);
    let ly = pf(6u);
    let k = pf(7u);
    let rr = pf(8u);
    let qx = lx + (x - lx) * k;
    let qy = ly + (y - ly) * k;
    let d = sample_fld(qx * pf(9u), qy * pf(9u)) * f32(pu(4u));
    if (d >= rr) { return 0.0; }
    let t = clamp((rr - d) / (2.0 * rr), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t) * pf(10u);
}

// occlusion x strength as u16 (x 65535, rounded), two pixels per word
@compute @workgroup_size(64)
fn sh_occ(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let q = lin(id, nwg);
    let n = pu(0u) * pu(1u);
    if (2u * q >= n) { return; }
    let a = u32(clamp(occ_at(2u * q), 0.0, 1.0) * 65535.0 + 0.5);
    var b = 0u;
    if (2u * q + 1u < n) { b = u32(clamp(occ_at(2u * q + 1u), 0.0, 1.0) * 65535.0 + 0.5); }
    occ[q] = a | (b << 16u);
}
