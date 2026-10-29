// Shared helpers for reel's compute kernels (prepended to every module).
// Binding 0 is the parameter block: 32-bit words, floats bit-cast (see gpu::Params).
// Pixels are premultiplied RGBA8 packed little-endian into u32 (r = bits 0..7).
@group(0) @binding(0) var<storage, read_write> P: array<u32>;

fn pu(i: u32) -> u32 { return P[i]; }
fn pf(i: u32) -> f32 { return bitcast<f32>(P[i]); }
fn pi(i: u32) -> i32 { return bitcast<i32>(P[i]); }

fn ch(p: u32, c: u32) -> u32 { return (p >> (8u * c)) & 255u; }
fn pack(r: u32, g: u32, b: u32, a: u32) -> u32 { return r | (g << 8u) | (b << 16u) | (a << 24u); }
// Rust `x as u8` for a float: truncation toward zero, saturating at 0 and 255.
fn to_u8(x: f32) -> u32 { return u32(clamp(x, 0.0, 255.0)); }
// Rust f32::round (half away from zero); WGSL round() is half-to-even.
fn round_away(x: f32) -> f32 { return sign(x) * floor(abs(x) + 0.5); }

// look.rs hash / unit / value_noise / fbm (bit-for-bit the integer parts).
fn hash(x: i32, y: i32, seed: u32) -> u32 {
    var h = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(y) * 0xd8163841u) ^ (seed * 0xcb1ab31fu) ^ 0x9e3779b9u;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn unit(h: u32) -> f32 { return f32(h >> 8u) * (1.0 / 16777216.0); }

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let xf = floor(x);
    let yf = floor(y);
    let ix = i32(xf);
    let iy = i32(yf);
    let tx = x - xf;
    let ty = y - yf;
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let a = unit(hash(ix, iy, seed));
    let b = unit(hash(ix + 1, iy, seed));
    let c = unit(hash(ix, iy + 1, seed));
    let d = unit(hash(ix + 1, iy + 1, seed));
    return (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy;
}

fn smoothstep_(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// look::put: c = colour over white (straight), a = coverage; paper = (rgb, has).
fn put(c: vec3<f32>, a: f32, has_paper: bool, pp: vec3<f32>) -> u32 {
    if (has_paper) {
        let r = to_u8(clamp(pp.x * c.x * a, 0.0, 1.0) * 255.0 + 0.5);
        let g = to_u8(clamp(pp.y * c.y * a, 0.0, 1.0) * 255.0 + 0.5);
        let b = to_u8(clamp(pp.z * c.z * a, 0.0, 1.0) * 255.0 + 0.5);
        return pack(r, g, b, to_u8(clamp(a, 0.0, 1.0) * 255.0 + 0.5));
    }
    let m = clamp(min(min(c.x, c.y), c.z), 0.0, 1.0);
    let al = 1.0 - m;
    let r = to_u8(clamp(c.x - m, 0.0, al) * 255.0 + 0.5);
    let g = to_u8(clamp(c.y - m, 0.0, al) * 255.0 + 0.5);
    let b = to_u8(clamp(c.z - m, 0.0, al) * 255.0 + 0.5);
    return pack(r, g, b, to_u8(al * 255.0 + 0.5));
}

// look::unpremul: straight colour (capped at 1) and alpha; transparent -> white, 0.
struct Straight { c: vec3<f32>, a: f32 }
fn unpremul(p: u32) -> Straight {
    let a8 = ch(p, 3u);
    if (a8 == 0u) { return Straight(vec3<f32>(1.0), 0.0); }
    let inv = 1.0 / f32(a8);
    return Straight(vec3<f32>(min(f32(ch(p, 0u)) * inv, 1.0), min(f32(ch(p, 1u)) * inv, 1.0), min(f32(ch(p, 2u)) * inv, 1.0)), f32(a8) / 255.0);
}

// 2D invocation index -> linear (dispatches may be folded into rows of 65535 groups).
fn lin(id: vec3<u32>, nwg: vec3<u32>) -> u32 { return id.x + id.y * nwg.x * 64u; }
