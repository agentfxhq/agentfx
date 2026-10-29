// effects::blur_full on the GPU, bit-exact: three horizontal then three vertical box
// passes (window clamped to the buffer, outside = transparent), each output channel
// ((acc * norm + 2^31) >> 32).min(255) with norm = floor(2^32 / (2r + 1)) in 64-bit
// integer arithmetic (emulated with 32-bit halves).
// P: [0] w, [1] h, [2] r, [3] norm
@group(0) @binding(1) var<storage, read_write> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

// (a * b + 2^31) >> 32 for a < 2^32, b < 2^32
fn mulshift(a: u32, b: u32) -> u32 {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    let lo = (p00 & 0xffffu) | (mid << 16u);
    var hi = p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    let lo2 = lo + 0x80000000u;
    if (lo2 < lo) { hi += 1u; }
    return hi;
}

fn box_at(x: u32, y: u32, horiz: bool) -> u32 {
    let w = pu(0u);
    let h = pu(1u);
    let r = pu(2u);
    let norm = pu(3u);
    if (r == 0u) { return src[y * w + x]; }
    var acc = vec4<u32>(0u);
    let n = select(h, w, horiz);
    let c = select(y, x, horiz);
    let lo = select(0u, c - r, c >= r);
    let hi = min(n - 1u, c + r);
    for (var i = lo; i <= hi; i++) {
        var p: u32;
        if (horiz) { p = src[y * w + i]; } else { p = src[i * w + x]; }
        acc += vec4<u32>(p & 255u, (p >> 8u) & 255u, (p >> 16u) & 255u, p >> 24u);
    }
    return pack(min(mulshift(acc.x, norm), 255u), min(mulshift(acc.y, norm), 255u), min(mulshift(acc.z, norm), 255u), min(mulshift(acc.w, norm), 255u));
}

@compute @workgroup_size(16, 16)
fn box_h(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= pu(0u) || id.y >= pu(1u)) { return; }
    dst[id.y * pu(0u) + id.x] = box_at(id.x, id.y, true);
}

@compute @workgroup_size(16, 16)
fn box_v(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= pu(0u) || id.y >= pu(1u)) { return; }
    dst[id.y * pu(0u) + id.x] = box_at(id.x, id.y, false);
}
