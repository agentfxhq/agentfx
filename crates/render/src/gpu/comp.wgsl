// effects.rs pieces on the GPU, integer-exact: tinted alpha (drop shadow, glow), shifted
// source-over, and the downsample / bilinear upsample of effects::blur's large-sigma path.
// P: 0 w, 1 h, 2..5 tint k (u32, x65536), 6 dx (i32), 7 dy (i32), 8 scale, 9 sw, 10 sh
@group(0) @binding(1) var<storage, read_write> px: array<u32>;
@group(0) @binding(2) var<storage, read_write> sh: array<u32>;

// sh = effects::tint_alpha(px)
@compute @workgroup_size(16, 16)
fn tint(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let al = ch(px[i], 3u);
    sh[i] = pack(min((al * pu(2u) + 32768u) >> 16u, 255u), min((al * pu(3u) + 32768u) >> 16u, 255u), min((al * pu(4u) + 32768u) >> 16u, 255u), min((al * pu(5u) + 32768u) >> 16u, 255u));
}

// px = px over effects::shift(sh, dx, dy)
@compute @workgroup_size(16, 16)
fn over_shift(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (id.x >= w || id.y >= h) { return; }
    let i = id.y * w + id.x;
    let sx = i32(id.x) - pi(6u);
    let sy = i32(id.y) - pi(7u);
    var u = 0u;
    if (sx >= 0 && sy >= 0 && sx < i32(w) && sy < i32(h)) { u = sh[u32(sy) * w + u32(sx)]; }
    let t = px[i];
    let ia = 255u - ch(t, 3u);
    px[i] = pack(min(ch(t, 0u) + (ch(u, 0u) * ia + 127u) / 255u, 255u), min(ch(t, 1u) + (ch(u, 1u) * ia + 127u) / 255u, 255u), min(ch(t, 2u) + (ch(u, 2u) * ia + 127u) / 255u, 255u), min(ch(t, 3u) + (ch(u, 3u) * ia + 127u) / 255u, 255u));
}

// sh (sw x sh) = scale x scale block means of px (partial blocks count transparent pixels)
@compute @workgroup_size(16, 16)
fn down(@builtin(global_invocation_id) id: vec3<u32>) {
    let sw = pu(9u);
    if (id.x >= sw || id.y >= pu(10u)) { return; }
    let w = pu(0u);
    let h = pu(1u);
    let s = pu(8u);
    var sum = vec4<u32>(0u);
    for (var y = id.y * s; y < min((id.y + 1u) * s, h); y++) {
        for (var x = id.x * s; x < min((id.x + 1u) * s, w); x++) {
            let p = px[y * w + x];
            sum += vec4<u32>(p & 255u, (p >> 8u) & 255u, (p >> 16u) & 255u, p >> 24u);
        }
    }
    let area = s * s;
    sum = (sum + vec4<u32>(area / 2u)) / area;
    sh[id.y * sw + id.x] = pack(sum.x, sum.y, sum.z, sum.w);
}

// px = bilinear (centre-aligned) upsample of sh; f32 is exact here for scale <= 128
// (dyadic weights), so this rounds like the CPU's f64 path
@compute @workgroup_size(16, 16)
fn up(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let s = f32(pu(8u));
    let sw = i32(pu(9u));
    let shh = i32(pu(10u));
    let fy = (f32(id.y) + 0.5) / s - 0.5;
    let fx = (f32(id.x) + 0.5) / s - 0.5;
    let iy = i32(floor(fy));
    let ix = i32(floor(fx));
    let ty = fy - f32(iy);
    let tx = fx - f32(ix);
    var v = vec4<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        let sy = iy + j;
        let wy = select(1.0 - ty, ty, j == 1);
        for (var k = 0; k < 2; k++) {
            let sx = ix + k;
            let wx = select(1.0 - tx, tx, k == 1);
            if (sx >= 0 && sy >= 0 && sx < sw && sy < shh) {
                let p = sh[u32(sy * sw + sx)];
                v += vec4<f32>(f32(p & 255u), f32((p >> 8u) & 255u), f32((p >> 16u) & 255u), f32(p >> 24u)) * wx * wy;
            }
        }
    }
    px[id.y * w + id.x] = pack(to_u8(round_away(v.x)), to_u8(round_away(v.y)), to_u8(round_away(v.z)), to_u8(round_away(v.w)));
}
