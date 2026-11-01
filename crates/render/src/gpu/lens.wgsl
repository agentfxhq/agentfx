// Liquid glass and cinematic optics. The source is immutable for each pass, so the
// integer sampling and final u8 conversion match look.rs' CPU reference.
// glass P: 0 w, 1 h, 2 strength, 3 blur, 4 dispersion, 5 thickness, 6 highlight,
//          7 rim, 8..11 tint rgba (tint alpha is zero when absent)
// optics P: 0 w, 1 h, 2 streak, 3 bokeh, 4 distortion, 5 aberration, 6 vignette,
//           7 halation, 8 grain, 9 seed, 10 ox, 11 oy, 12 frame
@group(0) @binding(1) var<storage, read_write> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
@group(0) @binding(3) var<storage, read_write> backdrop: array<u32>;

fn at(x: f32, y: f32) -> u32 {
    let ix = u32(clamp(round_away(x), 0.0, f32(pu(0u) - 1u)));
    let iy = u32(clamp(round_away(y), 0.0, f32(pu(1u) - 1u)));
    return src[iy * pu(0u) + ix];
}

fn back_at(x: f32, y: f32) -> u32 {
    let ix = u32(clamp(round_away(x), 0.0, f32(pu(0u) - 1u)));
    let iy = u32(clamp(round_away(y), 0.0, f32(pu(1u) - 1u)));
    return backdrop[iy * pu(0u) + ix];
}

@compute @workgroup_size(16, 16)
fn glass(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    if (id.x >= w || id.y >= pu(1u)) { return; }
    let i = id.y * w + id.x;
    let here = src[i];
    let max_alpha = max(pf(12u), 1.0);
    let alpha = f32(ch(here, 3u)) / max_alpha;
    let x = f32(id.x);
    let y = f32(id.y);
    let bevel = clamp(max(pf(5u), 0.0) * 8.0, 2.0, 32.0);
    let ax = (f32(ch(at(x + bevel, y), 3u)) - f32(ch(at(x - bevel, y), 3u))) / (2.0 * max_alpha);
    let ay = (f32(ch(at(x, y + bevel), 3u)) - f32(ch(at(x, y - bevel), 3u))) / (2.0 * max_alpha);
    let d = pf(2u) * max(pf(5u), 0.0) * alpha;
    let edge = min(sqrt(ax * ax + ay * ay), 1.0);
    let specular = min(max(-ax * 0.7 - ay * 0.7, 0.0), 1.0) * max(pf(6u), 0.0);
    var out = vec3<u32>(0u);
    for (var c = 0u; c < 3u; c++) {
        let chroma = (f32(c) - 1.0) * pf(4u);
        var v = f32(ch(back_at(x - ax * d + chroma, y - ay * d), c));
        if (pf(3u) > 0.0) {
            v = (v + f32(ch(back_at(x - ax * d + pf(3u), y - ay * d), c)) + f32(ch(back_at(x - ax * d - pf(3u), y - ay * d), c))) / 3.0;
        }
        let ta = pf(11u);
        if (ta > 0.0) { v = v * (1.0 - ta) + 255.0 * pf(8u + c) * ta; }
        v += 255.0 * (specular + max(pf(7u), 0.0) * edge);
        out[c] = to_u8(v * alpha);
    }
    dst[i] = pack(out.x, out.y, out.z, to_u8(alpha * 255.0));
}

@compute @workgroup_size(16, 16)
fn optics(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (id.x >= w || id.y >= h) { return; }
    let i = id.y * w + id.x;
    let here = src[i];
    let nx = (f32(id.x) / f32(w) - 0.5) * 2.0;
    let ny = (f32(id.y) / f32(h) - 0.5) * 2.0;
    let r2 = min(nx * nx + ny * ny, 2.0);
    let sx = f32(id.x) + nx * r2 * pf(4u) * 30.0;
    let sy = f32(id.y) + ny * r2 * pf(4u) * 30.0;
    var out = vec3<u32>(0u);
    for (var c = 0u; c < 3u; c++) {
        let ca = (f32(c) - 1.0) * pf(5u) * r2 * 8.0;
        var v = f32(ch(at(sx + ca, sy), c));
        if (pf(3u) > 0.0) {
            v = (v + f32(ch(at(sx + pf(3u) * 3.0, sy + pf(3u) * 2.0), c)) + f32(ch(at(sx - pf(3u) * 3.0, sy - pf(3u) * 2.0), c))) / 3.0;
        }
        let warm0 = f32(max(max(ch(at(sx, sy), 0u), ch(at(sx, sy), 1u)), ch(at(sx, sy), 2u))) / 255.0;
        let warm4 = (f32(max(max(ch(at(sx - 4.0, sy), 0u), ch(at(sx - 4.0, sy), 1u)), ch(at(sx - 4.0, sy), 2u)))
            + f32(max(max(ch(at(sx + 4.0, sy), 0u), ch(at(sx + 4.0, sy), 1u)), ch(at(sx + 4.0, sy), 2u)))
            + f32(max(max(ch(at(sx, sy - 4.0), 0u), ch(at(sx, sy - 4.0), 1u)), ch(at(sx, sy - 4.0), 2u)))
            + f32(max(max(ch(at(sx, sy + 4.0), 0u), ch(at(sx, sy + 4.0), 1u)), ch(at(sx, sy + 4.0), 2u)))) / 255.0;
        let warm10 = (f32(max(max(ch(at(sx - 10.0, sy), 0u), ch(at(sx - 10.0, sy), 1u)), ch(at(sx - 10.0, sy), 2u)))
            + f32(max(max(ch(at(sx + 10.0, sy), 0u), ch(at(sx + 10.0, sy), 1u)), ch(at(sx + 10.0, sy), 2u)))
            + f32(max(max(ch(at(sx, sy - 10.0), 0u), ch(at(sx, sy - 10.0), 1u)), ch(at(sx, sy - 10.0), 2u)))
            + f32(max(max(ch(at(sx, sy + 10.0), 0u), ch(at(sx, sy + 10.0), 1u)), ch(at(sx, sy + 10.0), 2u)))) / 255.0;
        let halo = warm0 * 0.28 + warm4 * 0.11 + warm10 * 0.035;
        let streak_light = (f32(ch(at(sx - 12.0, sy), c)) + f32(ch(at(sx + 12.0, sy), c))) * 0.26
            + (f32(ch(at(sx - 32.0, sy), c)) + f32(ch(at(sx + 32.0, sy), c))) * 0.16
            + (f32(ch(at(sx - 76.0, sy), c)) + f32(ch(at(sx + 76.0, sy), c))) * 0.08;
        var warm_color = 0.04;
        if (c == 1u) { warm_color = 0.22; }
        if (c == 0u) { warm_color = 0.95; }
        v += halo * 255.0 * pf(7u) * warm_color;
        v += streak_light * pf(2u);
        let seed = pu(9u) ^ pu(12u);
        let n = unit(hash(pi(10u) + i32(id.x), pi(11u) + i32(id.y), seed)) - 0.5;
        v = v * (1.0 - pf(6u) * r2 * 0.55) + n * pf(8u) * 42.0;
        out[c] = to_u8(v);
    }
    dst[i] = pack(out.x, out.y, out.z, ch(here, 3u));
}
