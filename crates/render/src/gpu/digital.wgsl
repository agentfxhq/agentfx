// digital.rs rgbsplit / glitch / vhs (gather through digital::Tab, the host's sampling
// tables) and look.rs grain. Bindings: 1 source frame, 2 output frame, 3 unpremultiply
// table (65536 f32: c / a at a * 256 + c, exactly the CPU's division), 4 sampling table.
// Params: 0 w, 1 h, 2 mode, 3 amount (f32), 4 gw, 5 x0, 6 seed; grain: see `grain`.
@group(0) @binding(1) var<storage, read_write> SRC: array<u32>;
@group(0) @binding(2) var<storage, read_write> DST: array<u32>;
@group(0) @binding(3) var<storage, read_write> UNP: array<f32>;
@group(0) @binding(4) var<storage, read_write> TAB: array<i32>;

// digital::rgb: straight colour (transparent -> 0)
fn urgb(p: u32) -> vec3<f32> {
    let a = ch(p, 3u) * 256u;
    return vec3<f32>(UNP[a + ch(p, 0u)], UNP[a + ch(p, 1u)], UNP[a + ch(p, 2u)]);
}

fn luma(c: vec3<f32>) -> f32 { return c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722; }

// Rust `v.round() as u8` for 0 <= v <= 255 (half away from zero)
fn round_u8(v: f32) -> u32 {
    let f = floor(v);
    return u32(select(f, f + 1.0, v - f >= 0.5));
}

// digital::put: the colour clamped to [0, 1] at the pixel's own alpha
fn dput(c: vec3<f32>, a8: u32) -> u32 {
    let a = f32(a8);
    let q = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return pack(round_u8(q.x * a), round_u8(q.y * a), round_u8(q.z * a), a8);
}

@compute @workgroup_size(16, 16)
fn digital(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    let x = id.x;
    let y = id.y;
    if (x >= w || y >= h) { return; }
    let mode = pu(2u);
    let row = y * w;
    let me = SRC[row + x];
    let original = urgb(me);
    var c: vec3<f32>;
    if (mode == 0u) {
        // rgbsplit
        let r = urgb(SRC[u32(TAB[2u * w + y]) * w + u32(TAB[x])]);
        let b = urgb(SRC[u32(TAB[2u * w + h + y]) * w + u32(TAB[w + x])]);
        c = vec3<f32>(r.x, original.y, b.z);
    } else if (mode == 1u) {
        // glitch
        let k = w + 2u * h + 3u * (u32(TAB[w + y]) * pu(4u) + u32(TAB[x]));
        let fl = TAB[k];
        if ((fl & 1) != 0) {
            c = urgb(SRC[row + at(x, TAB[k + 1u], w)]);
            c.x = urgb(SRC[row + at(x, TAB[k + 2u], w)]).x;
            if ((fl & 2) != 0) {
                c = vec3<f32>(round_away(c.x * 3.0) / 3.0, round_away(c.y * 3.0) / 3.0, round_away(c.z * 3.0) / 3.0);
            }
        } else {
            c = urgb(SRC[row + at(x, TAB[w + h + y], w)]);
        }
    } else {
        // vhs
        let r0 = 12u * y;
        let lum = luma(urgb(SRC[row + at(x, TAB[r0 + 4u], w)]));
        var chroma = vec3<f32>(0.0);
        for (var k = 0u; k < 9u; k++) {
            let q = urgb(SRC[row + at(x, TAB[r0 + k], w)]);
            let l = luma(q);
            chroma = chroma + (q - vec3<f32>(l)) / 9.0;
        }
        let noise = (unit(hash(i32(x) + pi(5u), TAB[r0 + 9u], pu(6u))) - 0.5) * bitcast<f32>(TAB[r0 + 11u]);
        let scan = bitcast<f32>(TAB[r0 + 10u]);
        let amount = pf(3u);
        c = original + amount * ((vec3<f32>(lum) + chroma * 0.86) * scan + vec3<f32>(noise) - original);
    }
    DST[row + x] = dput(c, ch(me, 3u));
}

// x + d clamped to [0, w)
fn at(x: u32, d: i32, w: u32) -> u32 {
    return u32(clamp(i32(x) + d, 0, i32(w) - 1));
}

// look::grain in place on SRC. Params: 0 w, 1 h, 7 x0 (= ox - ax), 8 y0 (= oy - ay), 9 seed,
// 10 amt (f32, amount * 255), 11 size (f32), 12 fine (size <= 1)
@compute @workgroup_size(16, 16)
fn grain(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (id.x >= w || id.y >= h) { return; }
    let i = id.y * w + id.x;
    let p = SRC[i];
    let a8 = ch(p, 3u);
    if (a8 == 0u) { return; }
    let lx = i32(id.x) + pi(7u);
    let ly = i32(id.y) + pi(8u);
    let sd = pu(9u);
    let amt = pf(10u);
    var d: f32;
    if (pu(12u) != 0u) {
        let n = unit(hash(lx, ly, sd)) + unit(hash(lx, ly, sd ^ 0x3c3cu)) - 1.0;
        d = n * amt;
    } else {
        let size = pf(11u);
        let fx = f32(lx) / size;
        let fy = f32(ly) / size;
        let n = value_noise(fx, fy, sd) + value_noise(fx, fy, sd ^ 0x3c3cu) - 1.0;
        d = n * 1.6 * amt;
    }
    let a = f32(a8);
    d = d * a / 255.0;
    let r = round_u8(clamp(f32(ch(p, 0u)) + d, 0.0, a));
    let g = round_u8(clamp(f32(ch(p, 1u)) + d, 0.0, a));
    let b = round_u8(clamp(f32(ch(p, 2u)) + d, 0.0, a));
    SRC[i] = pack(r, g, b, a8);
}
