// Temporal-glitch history compositor. Every history frame is packed consecutively in
// `frames`; its selection rules intentionally mirror timefx_paint.rs.
// P: 0 w, 1 h, 2 count, 3 block edge, 4 intensity, 5 smear, 6 stutter, 7 rgb,
//    8 scanline, 9 seed, 10 origin x, 11 origin y, 12 frame number.
@group(0) @binding(1) var<storage, read_write> frames: array<u32>;
@group(0) @binding(2) var<storage, read_write> outp: array<u32>;

fn pick(a: i32, b: i32, seed: u32, salt: u32, count: u32) -> u32 {
    return 1u + hash(a, b, seed ^ salt) % (count - 1u);
}

@compute @workgroup_size(16, 16)
fn temporal_glitch(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (id.x >= w || id.y >= h) { return; }
    let i = id.y * w + id.x;
    let seed = pu(9u) ^ (pu(12u) * 0x9e3779b9u);
    let edge = max(pi(3u), 1);
    let bx = i32(floor(f32(pi(10u) + i32(id.x)) / f32(edge)));
    let by = i32(floor(f32(pi(11u) + i32(id.y)) / f32(edge)));
    var history = 0u;
    if (unit(hash(bx, by, seed)) < pf(5u)) { history = pick(bx, by, seed, 0x6319u, pu(2u)); }
    if (unit(hash(0, pi(11u) + i32(id.y), seed ^ 0xa51cu)) < pf(8u)) { history = pick(0, pi(11u) + i32(id.y), seed, 0x7f4au, pu(2u)); }
    if (unit(hash(0, 0, seed ^ 0x91e1u)) < pf(6u)) { history = pick(pi(12u), 0, seed, 0x8831u, pu(2u)); }
    let npx = w * h;
    let base = frames[i];
    var value = frames[history * npx + i];
    if (history == 0u) { value = base; }
    if (history != 0u && pf(7u) > 0.0) {
        var c = vec4<u32>(ch(value, 0u), ch(value, 1u), ch(value, 2u), ch(value, 3u));
        for (var k = 0u; k < 3u; k++) {
            let other = pick(bx + i32(k) * 17, by, seed, 0x1459u + k, pu(2u));
            c[k] = to_u8(f32(c[k]) * (1.0 - pf(7u)) + f32(ch(frames[other * npx + i], k)) * pf(7u) + 0.5);
        }
        value = pack(c.x, c.y, c.z, c.w);
    }
    outp[i] = pack(
        to_u8(f32(ch(base, 0u)) * (1.0 - pf(4u)) + f32(ch(value, 0u)) * pf(4u) + 0.5),
        to_u8(f32(ch(base, 1u)) * (1.0 - pf(4u)) + f32(ch(value, 1u)) * pf(4u) + 0.5),
        to_u8(f32(ch(base, 2u)) * (1.0 - pf(4u)) + f32(ch(value, 2u)) * pf(4u) + 0.5),
        to_u8(f32(ch(base, 3u)) * (1.0 - pf(4u)) + f32(ch(value, 3u)) * pf(4u) + 0.5));
}
