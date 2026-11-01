//! Cellular automata (parameters: `agentfx_ir::generator::KINDS`, "life"): 2D outer-totalistic
//! rules (`born` / `survive` neighbour-count bitmasks, Conway = B3/S23 = 8 / 12) on a torus,
//! or 1D elementary rules (`rule` 0..255, Wolfram numbering) scrolling upward. Seeded random
//! start; the step shown is `floor(t * speed)`, simulated from step 0 for every frame (pure
//! function of t). Cells are rounded squares coloured by age, dying cells fade over `trail`
//! steps. CPU only (`cpu` override).
use super::raster::{color, layer, neon, px_per_unit, unit_to_px};
use super::{Generator, Input, palette};
use tiny_skia::{FillRule, Paint, PathBuilder, Rect};

// parameter indices (declaration order)
const MODE: usize = 0;
const BORN: usize = 1;
const SURVIVE: usize = 2;
const RULE: usize = 3;
const ZOOM: usize = 4;
const DENSITY: usize = 5;
const SPEED: usize = 6;
const TRAIL: usize = 7;
const CSPEED: usize = 8;
const OFFSET: usize = 9;

/// Most steps simulated for one frame.
const MAX_STEPS: u64 = 20_000;

pub struct Life;

/// Cell state: age > 0 alive (steps alive), age <= 0 dead (-steps since death).
fn simulate2d(cols: usize, rows: usize, steps: u64, born: u32, survive: u32, density: f64, seed: u32, trail: i32) -> Vec<i32> {
    let mut a: Vec<i32> = (0..cols * rows)
        .map(|i| {
            let r = crate::look::unit(crate::look::hash((i % cols) as i32, (i / cols) as i32, seed)) as f64;
            if r < density { 1 } else { -(trail + 1) }
        })
        .collect();
    let mut b = a.clone();
    for _ in 0..steps {
        for y in 0..rows {
            let (ym, yp) = ((y + rows - 1) % rows, (y + 1) % rows);
            for x in 0..cols {
                let (xm, xp) = ((x + cols - 1) % cols, (x + 1) % cols);
                let alive = |xx: usize, yy: usize| (a[yy * cols + xx] > 0) as u32;
                let n = alive(xm, ym) + alive(x, ym) + alive(xp, ym) + alive(xm, y) + alive(xp, y) + alive(xm, yp) + alive(x, yp) + alive(xp, yp);
                let c = a[y * cols + x];
                b[y * cols + x] = if c > 0 {
                    if survive >> n & 1 == 1 { c + 1 } else { 0 }
                } else if born >> n & 1 == 1 {
                    1
                } else {
                    (c - 1).max(-(trail + 1))
                };
            }
        }
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// 1D: row y shows generation `steps - rows + 1 + y` (newest at the bottom, rows before
/// generation 0 empty); age = run length of the cell's column being alive.
fn simulate1d(cols: usize, rows: usize, steps: u64, rule: u32, density: f64, seed: u32, trail: i32) -> Vec<i32> {
    let mut row: Vec<bool> = if density <= 0.0 {
        (0..cols).map(|x| x == cols / 2).collect()
    } else {
        (0..cols).map(|x| (crate::look::unit(crate::look::hash(x as i32, 0, seed)) as f64) < density).collect()
    };
    let mut age: Vec<i32> = row.iter().map(|&v| if v { 1 } else { -(trail + 1) }).collect();
    let total = steps + 1;
    let mut hist: std::collections::VecDeque<Vec<i32>> = std::collections::VecDeque::with_capacity(rows + 1);
    for _ in 0..total {
        hist.push_back(age.clone());
        if hist.len() > rows {
            hist.pop_front();
        }
        let next: Vec<bool> = (0..cols)
            .map(|x| {
                let l = row[(x + cols - 1) % cols] as u32;
                let c = row[x] as u32;
                let r = row[(x + 1) % cols] as u32;
                rule >> (l << 2 | c << 1 | r) & 1 == 1
            })
            .collect();
        for x in 0..cols {
            age[x] = if next[x] { if age[x] > 0 { age[x] + 1 } else { 1 } } else if age[x] > 0 { 0 } else { (age[x] - 1).max(-(trail + 1)) };
        }
        row = next;
    }
    let mut out = vec![-(trail + 1); cols * rows];
    let off = rows - hist.len();
    for (y, r) in hist.iter().enumerate() {
        out[(y + off) * cols..(y + off + 1) * cols].copy_from_slice(r);
    }
    out
}

impl Generator for Life {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        let (w, h) = (g.w, g.h);
        let Some(mut core) = layer(w, h) else { return };
        let rows = ((72.0 / g.p[ZOOM]).round() as usize).clamp(2, 2000);
        let cols = ((rows as f64 * g.aspect).round() as usize).clamp(2, 4000);
        let steps = ((g.t * g.p[SPEED]).max(0.0).floor() as u64).min(MAX_STEPS);
        let trail = g.p[TRAIL].round().max(0.0) as i32;
        let seed = g.seed as u32;
        let density = g.p[DENSITY];
        let cells = if g.p[MODE].round() as i32 == 1 {
            simulate1d(cols, rows, steps, g.p[RULE].round() as u32, density, seed, trail)
        } else {
            simulate2d(cols, rows, steps, g.p[BORN].round() as u32, g.p[SURVIVE].round() as u32, density, seed, trail)
        };
        // cell (x, y) covers unit [-aspect + x s, -1 + y s] with s = 2 / rows (fills the rect height)
        let s = 2.0 / rows as f64;
        let x0 = -(cols as f64) * s * 0.5;
        let gap = (0.12 * s) as f32;
        let tr = unit_to_px(g.m);
        let (cs, co) = (g.p[CSPEED] as f32, g.p[OFFSET] as f32);
        // one path per colour bin (age) and fade level (dead cells)
        const AGES: i32 = 24;
        let mut bins: Vec<Option<PathBuilder>> = (0..(AGES + trail) as usize).map(|_| None).collect();
        for y in 0..rows {
            for x in 0..cols {
                let a = cells[y * cols + x];
                // dead -a steps ago: a ghost while -a < trail
                if a <= 0 && -a >= trail {
                    continue;
                }
                let k = if a > 0 { (a.min(AGES) - 1) as usize } else { (AGES - a) as usize };
                let (px, py) = ((x0 + x as f64 * s) as f32, (-1.0 + y as f64 * s) as f32);
                if let Some(r) = Rect::from_xywh(px + gap * 0.5, py + gap * 0.5, s as f32 - gap, s as f32 - gap) {
                    bins[k].get_or_insert_with(PathBuilder::new).push_rect(r);
                }
            }
        }
        let mut paint = Paint::default();
        paint.anti_alias = true;
        for (k, pb) in bins.into_iter().enumerate() {
            let Some(path) = pb.and_then(|p| p.finish()) else { continue };
            let k = k as i32;
            let (pos, a) = if k < AGES {
                // newborn at a third of the palette, older cells move through it
                (0.66f32.mul_add(k as f32 / AGES as f32, 0.33), 1.0)
            } else {
                // dead for (k - AGES) steps: a dim ghost fading out
                let d = (k - AGES) as f64;
                (0.17, 0.45 * (1.0 - d / trail as f64).powi(2))
            };
            let c = palette(g.pal, cs.mul_add(pos, co));
            let mx = c[0].max(c[1]).max(c[2]).max(0.02);
            let c = [c[0] / mx, c[1] / mx, c[2] / mx, c[3]];
            paint.set_color(color(c, a));
            core.fill_path(&path, &paint, FillRule::Winding, tr, None);
        }
        neon(&core, out, w, h, 0.25 * s * px_per_unit(g.m), 0.5);
    }
}
