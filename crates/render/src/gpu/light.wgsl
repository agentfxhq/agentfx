// light::shadow and light::rays on the GPU.
//
// shadow: the SDF comes from the CPU (exact EDT); this is the per-pixel cone trace.
// P: 0 w, 1 h, 2 sw, 3 sh, 4 ds, 5 lx, 6 ly, 7 k, 8 rr, 9 inv_ds, 10 strength, 11..13 col
//
// rays: rays_bbox (alpha >= 128, atomics) -> rays_mask (half-res holes inside the box)
// -> rays_march (96 decaying samples toward the light, as u8 in all four channels)
// -> blur.wgsl (sigma 1.2, bit-exact) -> rays_final (exposure, additive light).
// P: 0 w, 1 h, 2 sw, 3 sh, 4 ds, 5 lx, 6 ly (half-res px), 7 len, 8 norm, 9 decay,
//    10 ox, 11 oy, 12 strength, 13..15 tint
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> field: array<f32>;
@group(0) @binding(3) var<storage, read_write> bbox: array<atomic<u32>, 4>;
// rays: [0..bw*bh] block occupancy of the mask, then its summed-area table
// ((bw+1) x (bh+1)); P 16 bw, 17 bh, 18 per-sample skip
@group(0) @binding(5) var<storage, read_write> blk: array<u32>;
@group(0) @binding(4) var<storage, read_write> lum8: array<u32>;

fn sample_field(x: f32, y: f32) -> f32 {
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
    let a = field[iy * w + ix];
    let b = field[iy * w + ix1];
    let c = field[iy1 * w + ix];
    let d = field[iy1 * w + ix1];
    return (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty;
}

@compute @workgroup_size(16, 16)
fn shadow(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let lx = pf(5u);
    let ly = pf(6u);
    let k = pf(7u);
    let rr = pf(8u);
    let inv_ds = pf(9u);
    let x = f32(id.x) + 0.5;
    let y = f32(id.y) + 0.5;
    let qx = lx + (x - lx) * k;
    let qy = ly + (y - ly) * k;
    let d = sample_field(qx * inv_ds, qy * inv_ds) * f32(pu(4u));
    if (d >= rr) { return; }
    let t = clamp((rr - d) / (2.0 * rr), 0.0, 1.0);
    let occ = t * t * (3.0 - 2.0 * t) * pf(10u);
    let o = px[i];
    let ia = 1.0 - f32(ch(o, 3u)) / 255.0;
    if (ia <= 0.0) { return; }
    let sa = occ * ia;
    let r = to_u8(min(f32(ch(o, 0u)) + pf(11u) * sa * 255.0 + 0.5, 255.0));
    let g = to_u8(min(f32(ch(o, 1u)) + pf(12u) * sa * 255.0 + 0.5, 255.0));
    let b = to_u8(min(f32(ch(o, 2u)) + pf(13u) * sa * 255.0 + 0.5, 255.0));
    let a = to_u8(min(f32(ch(o, 3u)) + sa * 255.0 + 0.5, 255.0));
    px[i] = pack(r, g, b, a);
}

@compute @workgroup_size(16, 16)
fn rays_bbox(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    if (ch(px[id.y * w + id.x], 3u) >= 128u) {
        atomicMin(&bbox[0], id.x);
        atomicMin(&bbox[1], id.y);
        atomicMax(&bbox[2], id.x + 1u);
        atomicMax(&bbox[3], id.y + 1u);
    }
}

// mask (half res) into `field`
@compute @workgroup_size(16, 16)
fn rays_mask(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    let sh = pu(3u);
    if (id.x >= sw || id.y >= sh) { return; }
    let w = pu(0u);
    let h = pu(1u);
    let ds = pu(4u);
    var sum = 0u;
    for (var yy = id.y * ds; yy < min((id.y + 1u) * ds, h); yy++) {
        for (var xx = id.x * ds; xx < min((id.x + 1u) * ds, w); xx++) {
            sum += ch(px[yy * w + xx], 3u);
        }
    }
    let a = f32(sum) / f32(255u * ds * ds);
    let b0 = atomicLoad(&bbox[0]);
    let x = f32(id.x) + 0.5;
    let y = f32(id.y) + 0.5;
    let fd = f32(ds);
    var m = 0.0;
    if (b0 != 0xffffffffu) {
        let hb = vec4<f32>(f32(b0) / fd, f32(atomicLoad(&bbox[1])) / fd, f32(atomicLoad(&bbox[2])) / fd, f32(atomicLoad(&bbox[3])) / fd);
        if (x >= hb.x && x <= hb.z && y >= hb.y && y <= hb.w) { m = 1.0 - a; }
    }
    field[id.y * sw + id.x] = m;
}

// look::sample_plane over the mask (outside = 0)
fn mask_at(x: i32, y: i32) -> f32 {
    let w = i32(pu(2u));
    let h = i32(pu(3u));
    if (x < 0 || y < 0 || x >= w || y >= h) { return 0.0; }
    return field[u32(y * w + x)];
}

fn sample_mask(x: f32, y: f32) -> f32 {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let a = mask_at(ix, iy);
    let b = mask_at(ix + 1, iy);
    let c = mask_at(ix, iy + 1);
    let d = mask_at(ix + 1, iy + 1);
    return (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty;
}

// light::march_rays' empty-space skip: blocks of 8 mask px holding any non-zero mask in
// their pixels plus one px right and below (the bilinear footprint)
const RB: u32 = 8u;

@compute @workgroup_size(16, 16)
fn rays_occ(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    let sh = pu(3u);
    if (id.x >= sw || id.y >= sh) { return; }
    if (field[id.y * sw + id.x] == 0.0) { return; }
    let bw = pu(16u);
    for (var k = 0u; k < 4u; k++) {
        let xx = i32(id.x) - i32(k & 1u);
        let yy = i32(id.y) - i32(k >> 1u);
        if (xx >= 0 && yy >= 0) {
            blk[(u32(yy) / RB) * bw + u32(xx) / RB] = 1u;
        }
    }
}

// summed-area table of the occupancy: prefix sums along rows, then down columns (row 0
// and column 0 of the table stay 0 from the upload)
@compute @workgroup_size(64)
fn rays_sat_rows(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let y = lin(id, nwg);
    let bw = pu(16u);
    let bh = pu(17u);
    if (y >= bh) { return; }
    let s0 = bw * bh;
    var run = 0u;
    for (var x = 0u; x < bw; x++) {
        run += blk[y * bw + x];
        blk[s0 + (y + 1u) * (bw + 1u) + x + 1u] = run;
    }
}

@compute @workgroup_size(64)
fn rays_sat_cols(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let x = lin(id, nwg) + 1u;
    let bw = pu(16u);
    let bh = pu(17u);
    if (x > bw) { return; }
    let s0 = bw * bh;
    var run = 0u;
    for (var y = 1u; y <= bh; y++) {
        run += blk[s0 + y * (bw + 1u) + x];
        blk[s0 + y * (bw + 1u) + x] = run;
    }
}

fn sat_at(x: u32, y: u32) -> u32 { return blk[pu(16u) * pu(17u) + y * (pu(16u) + 1u) + x]; }

fn any_block(x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    let bw = pu(16u);
    let bh = pu(17u);
    let bx0 = min(u32(max(floor(x0 - 1.5), 0.0)) / RB, bw);
    let by0 = min(u32(max(floor(y0 - 1.5), 0.0)) / RB, bh);
    let bx1 = min((u32(max(floor(x1 + 1.5), -1.0) + 1.0) + RB - 1u) / RB, bw);
    let by1 = min((u32(max(floor(y1 + 1.5), -1.0) + 1.0) + RB - 1u) / RB, bh);
    if (bx1 <= bx0 || by1 <= by0) { return false; }
    return sat_at(bx1, by1) + sat_at(bx0, by0) > sat_at(bx1, by0) + sat_at(bx0, by1);
}

@compute @workgroup_size(16, 16)
fn rays_march(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(2u);
    if (id.x >= sw || id.y >= pu(3u)) { return; }
    let lx = pf(5u);
    let ly = pf(6u);
    let len = pf(7u);
    let decay = pf(9u);
    let x = f32(id.x) + 0.5;
    let y = f32(id.y) + 0.5;
    let dx = (lx - x) * len / 96.0;
    let dy = (ly - y) * len / 96.0;
    let j = unit(hash(i32(id.x) + pi(10u), i32(id.y) + pi(11u), 0x5EEDu));
    var sx = x + dx * j;
    var sy = y + dy * j;
    var sum = 0.0;
    var wgt = 1.0;
    let ex = sx + dx * 96.0;
    let ey = sy + dy * 96.0;
    // skipped samples add exactly 0: the sums are bitwise the full march's
    if (any_block(min(sx, ex), min(sy, ey), max(sx, ex), max(sy, ey))) {
        let bw = pu(16u);
        let fw = f32(sw);
        let fh = f32(pu(3u));
        for (var s = 0; s < 96; s++) {
            let ix = floor(sx - 0.5);
            let iy = floor(sy - 0.5);
            var live = ix >= -1.0 && iy >= -1.0 && ix < fw && iy < fh;
            if (live && ix >= 0.0 && iy >= 0.0 && pu(18u) != 0u) { live = blk[(u32(iy) / RB) * bw + u32(ix) / RB] != 0u; }
            if (live) { sum += sample_mask(sx, sy) * wgt; }
            wgt *= decay;
            sx += dx;
            sy += dy;
        }
    }
    let o = sum / pf(8u);
    let b = to_u8(min(o, 1.0) * 255.0 + 0.5);
    lum8[id.y * sw + id.x] = pack(b, b, b, b);
}

fn lum_at(x: i32, y: i32) -> f32 {
    let w = i32(pu(2u));
    let h = i32(pu(3u));
    if (x < 0 || y < 0 || x >= w || y >= h) { return 0.0; }
    return 1.0 - exp(-1.8 * f32(ch(lum8[u32(y * w + x)], 3u)) / 255.0);
}

@compute @workgroup_size(16, 16)
fn rays_final(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let fd = f32(pu(4u));
    let fx = (f32(id.x) + 0.5) / fd - 0.5;
    let fy = (f32(id.y) + 0.5) / fd - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let a0 = lum_at(ix, iy);
    let b0 = lum_at(ix + 1, iy);
    let c0 = lum_at(ix, iy + 1);
    let d0 = lum_at(ix + 1, iy + 1);
    let v = (a0 + (b0 - a0) * tx) + ((c0 + (d0 - c0) * tx) - (a0 + (b0 - a0) * tx)) * ty;
    let k = min(v * pf(12u), 1.0);
    if (k <= 0.0) { return; }
    let i = id.y * w + id.x;
    let o = px[i];
    let a = f32(ch(o, 3u)) / 255.0;
    let na = min(a + k * (1.0 - a), 1.0);
    let r = to_u8(min(f32(ch(o, 0u)) / 255.0 + k * pf(13u), na) * 255.0 + 0.5);
    let g = to_u8(min(f32(ch(o, 1u)) / 255.0 + k * pf(14u), na) * 255.0 + 0.5);
    let b = to_u8(min(f32(ch(o, 2u)) / 255.0 + k * pf(15u), na) * 255.0 + 0.5);
    px[i] = pack(r, g, b, to_u8(na * 255.0 + 0.5));
}
