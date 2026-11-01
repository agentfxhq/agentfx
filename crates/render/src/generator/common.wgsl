// Generator kernels (generator/mod.rs): prepended to every kind's WGSL, after gpu/common.wgsl.
// A kind defines `fn sample(u: f32, v: f32) -> vec4<f32>` (straight RGBA at a point of its space);
// this file provides the entry point (supersampling, premultiply, dither) and shared helpers.
// P: 0 w, 1 h, 2 ox, 3 oy (i32, device origin), 4-9 m (pixel -> kind space, a b c d e f),
//    10 aa, 11 palette stops, 12 seed, 13 t; parameters from 16 (`gp(i)`, declaration order)
@group(0) @binding(1) var<storage, read_write> pal: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> outp: array<u32>;

fn gp(i: u32) -> f32 { return pf(16u + i); }

// Cyclic palette (generator::palette).
fn palette(s: f32) -> vec4<f32> {
    let n = pu(11u);
    let x = (s - floor(s)) * f32(n);
    let i = min(u32(floor(x)), n - 1u);
    let f = x - floor(x);
    let a = pal[i];
    let b = pal[(i + 1u) % n];
    return a + (b - a) * f;
}

// Smooth escape count of z -> z^2 + c, -1e30 inside (generator::escape).
fn escape(z0: vec2<f32>, c: vec2<f32>, iter: u32, bail2: f32) -> f32 {
    var zr = z0.x;
    var zi = z0.y;
    for (var n = 0u; n < iter; n++) {
        let r2 = zr * zr;
        let i2 = zi * zi;
        if (r2 + i2 > bail2) {
            let lz = log2(r2 + i2) * 0.5;
            return f32(n) + 1.0 - log2(lz);
        }
        zi = fma(zr + zr, zi, c.y);
        zr = r2 - i2 + c.x;
    }
    return -1e30;
}

// generator::fractal_color (e = -1e30: inside, opaque black).
fn fractal_color(e: f32, speed: f32, offset: f32) -> vec4<f32> {
    if (e < -1e29) { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    return palette(offset + speed * log2(1.0 + max(e, 0.0)) * 0.5);
}

// generator::gnoise: 2D gradient noise, about [-1, 1], lattice spacing 1.
fn gn_corner(ix: i32, iy: i32, seed: u32, dx: f32, dy: f32) -> f32 {
    let h = hash(ix, iy, seed);
    let gx = fma(f32(h & 0xffffu), 1.0 / 32768.0, -1.0);
    let gy = fma(f32(h >> 16u), 1.0 / 32768.0, -1.0);
    return fma(gx, dx, gy * dy);
}
fn gn_fade(t: f32) -> f32 { return t * t * t * fma(t, fma(t, 6.0, -15.0), 10.0); }
fn gnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xf = floor(x);
    let yf = floor(y);
    let ix = i32(xf);
    let iy = i32(yf);
    let tx = x - xf;
    let ty = y - yf;
    let sx = gn_fade(tx);
    let sy = gn_fade(ty);
    let a = gn_corner(ix, iy, seed, tx, ty);
    let b = gn_corner(ix + 1, iy, seed, tx - 1.0, ty);
    let c = gn_corner(ix, iy + 1, seed, tx, ty - 1.0);
    let d = gn_corner(ix + 1, iy + 1, seed, tx - 1.0, ty - 1.0);
    let ab = fma(b - a, sx, a);
    let cd = fma(d - c, sx, c);
    return fma(cd - ab, sy, ab);
}

// generator::fbm
fn fbm(p0: vec2<f32>, oct: u32, seed: u32) -> f32 {
    var x = p0.x;
    var y = p0.y;
    var sum = 0.0;
    var amp = 0.5;
    for (var o = 0u; o < oct; o++) {
        sum = fma(amp, gnoise(x, y, seed + o * 7919u), sum);
        let nx = fma(1.6, x, 1.2 * y);
        let ny = fma(-1.2, x, 1.6 * y);
        x = nx;
        y = ny;
        amp = amp * 0.5;
    }
    return sum * 1.6;
}

@compute @workgroup_size(16, 16)
fn gen_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let aa = pu(10u);
    var acc = vec4<f32>(0.0);
    for (var sy = 0u; sy < aa; sy++) {
        for (var sx = 0u; sx < aa; sx++) {
            let px = f32(id.x) + (f32(sx) + 0.5) / f32(aa);
            let py = f32(id.y) + (f32(sy) + 0.5) / f32(aa);
            let u = fma(pf(4u), px, fma(pf(6u), py, pf(8u)));
            let v = fma(pf(5u), px, fma(pf(7u), py, pf(9u)));
            let c = sample(u, v);
            acc += vec4<f32>(c.rgb * c.a, c.a);
        }
    }
    let inv = 1.0 / f32(aa * aa);
    let d = unit(hash(pi(2u) + i32(id.x), pi(3u) + i32(id.y), pu(12u)));
    let o = acc * inv * 255.0 + vec4<f32>(d);
    outp[id.y * w + id.x] = pack(to_u8(o.r), to_u8(o.g), to_u8(o.b), to_u8(o.a));
}
