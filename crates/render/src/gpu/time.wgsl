// Time ring gather (E4 phase 2 step 3): every output pixel lerps two renders of the
// subtree at neighbouring grid times, taken from the ring (one storage buffer of slots).
// c = grid coordinate of the pixel's time (0 = the frame time, j = j grid steps back):
// per line (slit-scan, f32 table) or per pixel (displace, u16 map x kq).
// P: 0 w, 1 h (output), 2 mode (0 per column, 1 per row, 2 per pixel), 3 jlo (i32), 4 nj,
//    5 kq, 6 fw, 7 fh (ring slot size), 8 dx, 9 dy (output origin in the slot)
@group(0) @binding(1) var<storage, read_write> ring: array<u32>;
@group(0) @binding(2) var<storage, read_write> coord: array<u32>;
@group(0) @binding(3) var<storage, read_write> slots: array<u32>;
@group(0) @binding(4) var<storage, read_write> outp: array<u32>;

fn lerp8(a: u32, b: u32, fr: f32) -> u32 {
    let fa = f32(a);
    let d = (f32(b) - fa) * fr;
    return to_u8(fa + d + 0.5);
}

@compute @workgroup_size(16, 16)
fn ring_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let mode = pu(2u);
    var c: f32;
    if (mode == 0u) {
        c = bitcast<f32>(coord[id.x]);
    } else if (mode == 1u) {
        c = bitcast<f32>(coord[id.y]);
    } else {
        let q = (coord[i >> 1u] >> (16u * (i & 1u))) & 0xffffu;
        c = f32(q) * pf(5u);
    }
    let jf = floor(c);
    let fr = c - jf;
    let j = u32(i32(jf) - pi(3u));
    let fw = pu(6u);
    let npx = fw * pu(7u);
    let ri = (id.y + pu(9u)) * fw + id.x + pu(8u);
    let a = ring[slots[j] * npx + ri];
    if (fr <= 0.0) {
        outp[i] = a;
        return;
    }
    let b = ring[slots[j + 1u] * npx + ri];
    outp[i] = pack(lerp8(ch(a, 0u), ch(b, 0u), fr), lerp8(ch(a, 1u), ch(b, 1u), fr), lerp8(ch(a, 2u), ch(b, 2u), fr), lerp8(ch(a, 3u), ch(b, 3u), fr));
}
