//! Generator kinds (`Content::Gen`): the declared parameters of every kind (names, defaults,
//! ranges) and the named palettes. Shared by IR validation, the Reel Script compiler (parsing,
//! lint, catalog) and the renderer (`agentfx-render/src/generator/`, which holds the CPU and GPU
//! implementations). Adding a kind: one `Kind` entry in `KINDS` here, see docs/GENERATORS.md.

/// One numeric parameter of a generator kind. Animatable through a `gen.<name>` track.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub doc: &'static str,
}

/// A generator kind as the IR, the compiler and the catalog see it.
#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub name: &'static str,
    pub doc: &'static str,
    /// Parameters in a fixed order (the renderer passes them to the CPU and GPU code in this order).
    pub params: &'static [Param],
    /// The renderer has a GPU kernel for it (otherwise it always runs on the CPU).
    pub gpu: bool,
    /// Largest effective zoom (`zoom * 2^depth`) the f32 GPU kernel resolves (lint W_GEN_ZOOM).
    pub gpu_zoom_max: Option<f64>,
    /// The image changes with time itself (not only through parameter tracks).
    pub time: bool,
}

const fn p(name: &'static str, default: f64, min: f64, max: f64, doc: &'static str) -> Param {
    Param { name, default, min, max, doc }
}

const ZOOM: Param = p("zoom", 1.0, 1e-6, 1e15, "magnification (1 = the whole set fits the height)");
const DEPTH: Param = p("depth", 0.0, -40.0, 60.0, "extra zoom in octaves: effective zoom = zoom * 2^depth (animate linearly for a constant-speed dive)");
const TURN: Param = p("turn", 0.0, -1e6, 1e6, "rotation of the view, degrees");
const ITER: Param = p("iter", 300.0, 1.0, 20000.0, "maximum iterations");
const BAILOUT: Param = p("bailout", 64.0, 2.0, 1e6, "escape radius (large values give smoother colour)");
const SPEED: Param = p("color-speed", 1.0, -100.0, 100.0, "palette cycles per unit of log2 escape count");
const OFFSET: Param = p("color-offset", 0.0, -1e6, 1e6, "palette phase (animate to cycle the colours)");
const AA: Param = p("aa", 2.0, 1.0, 4.0, "supersampling: samples per pixel axis (1..4)");
/// The view of the pattern kinds (noise, tilings): centre and magnification.
const RE0: Param = p("re", 0.0, -1e6, 1e6, "view centre, horizontal (pattern units; animate to pan)");
const IM0: Param = p("im", 0.0, -1e6, 1e6, "view centre, vertical (pattern units; animate to pan)");
const ZOOM1: Param = p("zoom", 1.0, 1e-3, 1e3, "magnification (larger = bigger features)");
const GROW: Param = p("grow", 1.0, 0.0, 1.0, "draw-on progress 0..1 (animate 0 -> 1)");
const FLOW: Param = p("flow", 0.25, -100.0, 100.0, "how fast the pattern evolves with time (per second)");

/// Every kind the renderer implements.
pub const KINDS: &[Kind] = &[
    Kind {
        name: "julia",
        doc: "Julia set z -> z^2 + c with smooth escape-time colouring; animate cx/cy to make it breathe",
        params: &[
            p("cx", -0.8, -4.0, 4.0, "real part of c"),
            p("cy", 0.156, -4.0, 4.0, "imaginary part of c"),
            p("re", 0.0, -4.0, 4.0, "view centre, real axis"),
            p("im", 0.0, -4.0, 4.0, "view centre, imaginary axis"),
            ZOOM, DEPTH, TURN, ITER, BAILOUT, SPEED, OFFSET, AA,
        ],
        gpu: true,
        gpu_zoom_max: Some(2e4),
        time: false,
    },
    Kind {
        name: "mandelbrot",
        doc: "Mandelbrot set with smooth escape-time colouring; CPU in f64 (deep zooms), GPU in f32 (to about 2e4x)",
        params: &[
            p("re", -0.6, -4.0, 4.0, "view centre, real axis"),
            p("im", 0.0, -4.0, 4.0, "view centre, imaginary axis"),
            ZOOM, DEPTH, TURN, ITER, BAILOUT, SPEED, OFFSET, AA,
        ],
        gpu: true,
        gpu_zoom_max: Some(2e4),
        time: false,
    },
    Kind {
        name: "noise",
        doc: "Domain-warped fBm (two-level warp, Inigo Quilez): marble, oil, liquid neon; evolves with time",
        params: &[
            RE0, IM0, ZOOM1, TURN,
            p("warp", 2.5, 0.0, 20.0, "strength of the domain warp (0 = plain fBm, 1-4 liquid, 8+ turbulent)"),
            p("octaves", 4.0, 1.0, 8.0, "fBm octaves (detail)"),
            FLOW,
            p("color-speed", 1.0, -100.0, 100.0, "palette cycles across the value range (higher = more colour bands)"),
            OFFSET, AA,
        ],
        gpu: true,
        gpu_zoom_max: None,
        time: true,
    },
    Kind {
        name: "flow",
        doc: "Glowing particle streaks through an evolving curl-noise field (additive, bloom, transparent background); CPU",
        params: &[
            RE0, IM0,
            p("zoom", 1.0, 1e-3, 1e3, "field magnification (larger = wider swirls)"),
            TURN,
            p("particles", 1500.0, 1.0, 20000.0, "number of particles"),
            p("trail", 0.6, 0.0, 10.0, "trail length (unit space: 2 = the rect height)"),
            p("flow", 0.08, -100.0, 100.0, "how fast the field itself evolves (per second)"),
            p("speed", 0.5, 0.0, 100.0, "particle speed (unit space per second; keep constant, animate flow instead)"),
            p("thickness", 0.006, 0.0005, 0.2, "line width (unit space)"),
            p("color-speed", 1.0, -100.0, 100.0, "palette spread across the field (0 = one colour)"),
            OFFSET,
        ],
        gpu: false,
        gpu_zoom_max: None,
        time: true,
    },
    Kind {
        name: "truchet",
        doc: "Truchet tiling of glowing quarter-circle arcs; animate phase to twist the tiles (staggered quarter turns)",
        params: &[
            RE0, IM0,
            p("zoom", 1.0, 1e-3, 1e3, "magnification (1 = 8 tiles over the rect height)"),
            TURN,
            p("thickness", 0.06, 0.002, 0.5, "line width (tile units)"),
            p("phase", 0.0, -1e4, 1e4, "tile twist: every whole step turns each tile a quarter, staggered (animate 0 -> 1 -> 2 ...)"),
            p("mode", 0.0, 0.0, 1.0, "0 = one arc pair per tile, 1 = double arcs (parallel lines, a woven look)"),
            p("color-speed", 1.0, -100.0, 100.0, "palette cycles across the rect"),
            OFFSET, AA,
        ],
        gpu: true,
        gpu_zoom_max: None,
        time: false,
    },
    Kind {
        name: "voronoi",
        doc: "Voronoi cells with drifting seeds: shaded cells, glowing edges or faceted crystal; palette by cell",
        params: &[
            RE0, IM0,
            p("zoom", 1.0, 1e-3, 1e3, "magnification (1 = about 8 cells over the rect height)"),
            TURN,
            p("jitter", 1.0, 0.0, 1.0, "how far the seeds wander inside their cells (0 = square grid)"),
            p("flow", 0.15, -100.0, 100.0, "seed drift speed (cycles per second)"),
            p("mode", 0.0, 0.0, 2.0, "0 = shaded cells, 1 = glowing edges on black, 2 = crystal facets (F2 - F1)"),
            p("thickness", 0.04, 0.002, 0.5, "edge width (cell units)"),
            p("color-speed", 1.0, -100.0, 100.0, "palette spread over the cells"),
            OFFSET, AA,
        ],
        gpu: true,
        gpu_zoom_max: None,
        time: true,
    },
    Kind {
        name: "lsystem",
        doc: "L-system curves and plants drawn as glowing polylines (presets koch dragon sierpinski plant hilbert gosper); animate grow to draw them on; CPU",
        params: &[
            p("preset", 1.0, 0.0, 5.0, "0 koch (snowflake), 1 dragon, 2 sierpinski (arrowhead), 3 plant, 4 hilbert, 5 gosper"),
            p("iter", 0.0, 0.0, 16.0, "rewriting iterations (0 = the preset's own: koch 4, dragon 11, sierpinski 7, plant 6, hilbert 6, gosper 4)"),
            GROW,
            p("thickness", 0.008, 0.0005, 0.2, "line width (unit space; plant branches thin out with depth)"),
            p("re", 0.0, -100.0, 100.0, "offset, horizontal (unit space)"),
            p("im", 0.0, -100.0, 100.0, "offset, vertical (unit space)"),
            p("zoom", 1.0, 1e-3, 1e3, "magnification (1 = fitted into the rect)"),
            TURN,
            p("color-speed", 1.0, -100.0, 100.0, "palette cycles along the path (root to tip)"),
            OFFSET,
        ],
        gpu: false,
        gpu_zoom_max: None,
        time: false,
    },
    Kind {
        name: "penrose",
        doc: "Penrose P3 rhombus tiling by deflation (Robinson triangles): palette-filled thick/thin rhombi with glowing edges; animate grow to spread it from the centre; CPU",
        params: &[
            p("depth", 6.0, 0.0, 10.0, "deflation steps (more = smaller tiles; 10 is about 100k triangles)"),
            GROW,
            p("thickness", 0.004, 0.0, 0.2, "edge width (unit space; 0 = no edges)"),
            p("re", 0.0, -100.0, 100.0, "offset, horizontal (unit space)"),
            p("im", 0.0, -100.0, 100.0, "offset, vertical (unit space)"),
            p("zoom", 1.0, 1e-3, 1e3, "magnification about the centre of the sun (1 = the tiling covers the rect)"),
            TURN,
            p("color-speed", 1.0, -100.0, 100.0, "palette cycles from the centre to the rim (rings of colour)"),
            OFFSET,
        ],
        gpu: false,
        gpu_zoom_max: None,
        time: false,
    },
    Kind {
        name: "life",
        doc: "Cellular automata: Game of Life and any B/S rule (rule=\"B36/S23\"), or 1D elementary rules (mode=1 rule=30); seeded start, steps advance with time; CPU",
        params: &[
            p("mode", 0.0, 0.0, 1.0, "0 = 2D (born/survive), 1 = 1D elementary rule scrolling upward"),
            p("born", 8.0, 0.0, 511.0, "2D: bitmask of neighbour counts that give birth (bit n = n neighbours; B3 = 8). Script: rule=\"B3/S23\""),
            p("survive", 12.0, 0.0, 511.0, "2D: bitmask of neighbour counts that survive (S23 = 4 + 8 = 12)"),
            p("rule", 30.0, 0.0, 255.0, "1D: elementary rule number (30, 90, 110, 184, ...)"),
            p("zoom", 1.0, 0.02, 30.0, "cell size (1 = 72 cells over the rect height)"),
            p("density", 0.3, 0.0, 1.0, "share of cells alive at the start (1D with 0: one centre cell)"),
            p("speed", 12.0, 0.0, 240.0, "steps per second"),
            p("trail", 8.0, 0.0, 64.0, "steps a dead cell takes to fade out (0 = none)"),
            p("color-speed", 1.0, -100.0, 100.0, "palette cycles over a cell's age (0..24 steps)"),
            OFFSET,
        ],
        gpu: false,
        gpu_zoom_max: None,
        time: true,
    },
];

/// `lsystem` presets in `preset=` order (Reel Script also takes the names: `preset=dragon`).
pub const LSYSTEM_PRESETS: &[&str] = &["koch", "dragon", "sierpinski", "plant", "hilbert", "gosper"];

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name == name)
}

impl Kind {
    pub fn param(&self, name: &str) -> Option<&'static Param> {
        self.params.iter().find(|p| p.name == name)
    }
    pub fn index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }
}

/// Every parameter name of every kind (deduplicated, first-seen order).
pub fn param_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = vec![];
    for k in KINDS {
        for p in k.params {
            if !v.contains(&p.name) {
                v.push(p.name);
            }
        }
    }
    v
}

/// Most palette stops a generator takes (the GPU kernels read a fixed-size table).
pub const PALETTE_MAX: usize = 16;

/// Named palettes: sRGB hex stops, cyclic (the last stop blends back into the first).
pub const PALETTES: &[(&str, &[&str])] = &[
    ("neon", &["#06001a", "#2a0a8c", "#ff1fae", "#ffd36e", "#00e5ff", "#1b0466"]),
    ("vapor", &["#140832", "#ff71ce", "#fffb96", "#01cdfe", "#b967ff", "#3b0f6e"]),
    ("acid", &["#050505", "#39ff14", "#faff00", "#ff00a0", "#00ffd0", "#101010"]),
    ("ember", &["#040000", "#5c0a00", "#d43a00", "#ff9a1f", "#ffe6a8", "#7a1400"]),
    ("ice", &["#00040f", "#022a5c", "#1f7ae0", "#9be7ff", "#ffffff", "#0b3a7a"]),
    ("mono", &["#000000", "#ffffff"]),
];

/// The stops of a named palette (straight sRGB RGBA 0..1).
pub fn palette(name: &str) -> Option<Vec<[f64; 4]>> {
    let (_, stops) = PALETTES.iter().find(|(n, _)| *n == name)?;
    stops.iter().map(|s| hex(s)).collect()
}

/// `#rrggbb` / `#rrggbbaa` to straight RGBA 0..1.
pub fn hex(s: &str) -> Option<[f64; 4]> {
    let h = s.strip_prefix('#')?;
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f64 / 255.0);
    match h.len() {
        6 => Some([b(0)?, b(2)?, b(4)?, 1.0]),
        8 => Some([b(0)?, b(2)?, b(4)?, b(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds_are_well_formed() {
        for k in KINDS {
            for (i, p) in k.params.iter().enumerate() {
                assert!(p.min <= p.default && p.default <= p.max, "{}.{}", k.name, p.name);
                assert!(k.params[..i].iter().all(|q| q.name != p.name), "{}.{} twice", k.name, p.name);
            }
        }
        for (n, _) in PALETTES {
            let s = palette(n).unwrap();
            assert!(!s.is_empty() && s.len() <= PALETTE_MAX, "{n}");
        }
    }
}
