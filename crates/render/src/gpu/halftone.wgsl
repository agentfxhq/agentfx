// look::halftone on the GPU (the source blur runs before, in blur.wgsl or on the CPU).
// ht_tone: tone per screen per pixel of the blurred source; ht_grid: dot radius per
// rotated cell (tone sampled at the cell centre); ht_final: dot coverage per pixel.
// P: 0 w, 1 h, 2 ns, 3 cell, 4 l0x, 5 l0y, 6 has_paper, 7..9 paper rgb, 10 cmyk,
//    11 inv_pi, 12 total cells; screen si at 16 + 12 si: ink rgb, co, sn, u0, v0, gw, gh, roff
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> src: array<u32>;
@group(0) @binding(3) var<storage, read_write> tone: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> rgrid: array<f32>;

fn sp(si: u32, k: u32) -> u32 { return 16u + si * 12u + k; }

@compute @workgroup_size(16, 16)
fn ht_tone(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let st = unpremul(src[i]);
    let c = st.c;
    let a = st.a;
    if (pu(10u) != 0u) {
        let k = 1.0 - max(max(c.x, c.y), c.z);
        let d = max(1.0 - k, 1e-4);
        tone[i] = vec4<f32>((1.0 - c.x - k) / d * a, (1.0 - c.y - k) / d * a, (1.0 - c.z - k) / d * a, k * a);
    } else {
        let l = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        tone[i] = vec4<f32>((1.0 - l) * a, 0.0, 0.0, 0.0);
    }
}

fn tone_at(x: i32, y: i32, k: u32) -> f32 {
    let w = i32(pu(0u));
    let h = i32(pu(1u));
    if (x < 0 || y < 0 || x >= w || y >= h) { return 0.0; }
    return tone[u32(y * w + x)][k];
}

// look::sample_plane_n
fn sample_tone(k: u32, x: f32, y: f32) -> f32 {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let a = tone_at(ix, iy, k);
    let b = tone_at(ix + 1, iy, k);
    let c = tone_at(ix, iy + 1, k);
    let d = tone_at(ix + 1, iy + 1, k);
    return (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty;
}

@compute @workgroup_size(64)
fn ht_grid(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let q = lin(id, nwg);
    if (q >= pu(12u)) { return; }
    let ns = pu(2u);
    var si = 0u;
    for (var k = 1u; k < ns; k++) {
        if (q >= pu(sp(k, 9u))) { si = k; }
    }
    let ql = q - pu(sp(si, 9u));
    let gw = pu(sp(si, 7u));
    let co = pf(sp(si, 3u));
    let sn = pf(sp(si, 4u));
    let cell = pf(3u);
    let cu = f32(pi(sp(si, 5u)) + i32(ql % gw)) + 0.5;
    let cv = f32(pi(sp(si, 6u)) + i32(ql / gw)) + 0.5;
    let gx = (cu * co - cv * sn) * cell - pf(4u);
    let gy = (cu * sn + cv * co) * cell - pf(5u);
    let k = sample_tone(si, gx, gy);
    if (k <= 0.002) { rgrid[q] = -1.0; } else { rgrid[q] = cell * sqrt(min(k, 1.0) * pf(11u)) * 1.02; }
}

@compute @workgroup_size(16, 16)
fn ht_final(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let cell = pf(3u);
    let lx = f32(id.x) + pf(4u) + 0.5;
    let ly = f32(id.y) + pf(5u) + 0.5;
    var c = vec3<f32>(1.0);
    var cov_any = 0.0;
    for (var si = 0u; si < pu(2u); si++) {
        let ink = vec3<f32>(pf(sp(si, 0u)), pf(sp(si, 1u)), pf(sp(si, 2u)));
        let co = pf(sp(si, 3u));
        let sn = pf(sp(si, 4u));
        let u0 = pi(sp(si, 5u));
        let v0 = pi(sp(si, 6u));
        let gw = i32(pu(sp(si, 7u)));
        let gh = i32(pu(sp(si, 8u)));
        let roff = pu(sp(si, 9u));
        let u = (lx * co + ly * sn) / cell;
        let v = (-lx * sn + ly * co) / cell;
        let iu = i32(floor(u)) - u0;
        let iv = i32(floor(v)) - v0;
        var cov = 0.0;
        for (var dj = -1; dj <= 1; dj++) {
            let j = iv + dj;
            if (j < 0 || j >= gh) { continue; }
            for (var di = -1; di <= 1; di++) {
                let ii = iu + di;
                if (ii < 0 || ii >= gw) { continue; }
                let r = rgrid[roff + u32(j * gw + ii)];
                if (r <= 0.0) { continue; }
                let du = (u - f32(u0 + ii) - 0.5) * cell;
                let dv = (v - f32(v0 + j) - 0.5) * cell;
                let d2 = du * du + dv * dv;
                if (d2 >= (r + 0.5) * (r + 0.5)) { continue; }
                cov = max(cov, clamp(r - sqrt(d2) + 0.5, 0.0, 1.0));
            }
        }
        if (cov > 0.0) {
            cov_any = max(cov_any, cov);
            c *= vec3<f32>(1.0) - cov * (vec3<f32>(1.0) - ink);
        }
    }
    if (pu(6u) != 0u) {
        let a = f32(ch(px[i], 3u)) / 255.0;
        px[i] = put(c, max(a, cov_any), true, vec3<f32>(pf(7u), pf(8u), pf(9u)));
    } else {
        px[i] = put(c, 1.0, false, vec3<f32>(0.0));
    }
}
