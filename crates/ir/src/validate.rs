//! Structural and feature validation (fail loudly on anything the renderer cannot draw).
use crate::*;
use std::{io::Read, path::Path};

/// Check a scene for structural errors and unsupported features.
/// Returns warnings on success, or all errors. `base_dir` contains the IR file.
pub fn validate(scene: &Scene, base_dir: &Path) -> Result<Vec<String>, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if !IR_VERSIONS.contains(&scene.version) { errors.push(format!("version {} not in {IR_VERSIONS:?}", scene.version)); }
    for (name, value) in [("width", scene.width), ("height", scene.height), ("fps", scene.fps), ("frames", scene.frames)] {
        if value == 0 { errors.push(format!("{name} must be positive")); }
    }
    if scene.camera.is_some() { errors.push("camera is not implemented (planned)".into()); }
    let n = scene.nodes.len();
    if n == 0 { errors.push("nodes[0] root is missing".into()); }
    let mut parents = vec![0; n];
    for (i, node) in scene.nodes.iter().enumerate() {
        for &child in &node.children {
            if child >= n { errors.push(format!("node {i} {:?}: child {child} out of range", node.name)); }
            else { parents[child] += 1; }
        }
    }
    for (i, &count) in parents.iter().enumerate() {
        if (i == 0 && count != 0) || (i != 0 && count != 1) {
            errors.push(format!("node {i} {:?}: {count} parents; expected {}", scene.nodes[i].name, if i == 0 { 0 } else { 1 }));
        }
    }
    // Iterative DFS handles even malformed, deep graphs without stack overflow.
    let mut state = vec![0u8; n];
    let mut reachable = vec![false; n];
    for start in 0..n {
        if state[start] != 0 { continue; }
        let mut stack = vec![(start, false)];
        while let Some((i, exit)) = stack.pop() {
            if exit { state[i] = 2; continue; }
            if state[i] == 1 { errors.push(format!("node {i} {:?}: cycle detected", scene.nodes[i].name)); continue; }
            if state[i] == 2 { continue; }
            state[i] = 1;
            if start == 0 { reachable[i] = true; }
            stack.push((i, true));
            for &child in scene.nodes[i].children.iter().rev() {
                if child < n { stack.push((child, false)); }
            }
        }
    }
    for (i, &seen) in reachable.iter().enumerate() {
        if !seen { warnings.push(format!("node {i} {:?}: unreachable from root", scene.nodes[i].name)); }
    }
    for (i, track) in scene.tracks.iter().enumerate() {
        let ctx = format!("track {i} {:?}", track.label);
        if track.node >= n { errors.push(format!("{ctx}: node {} out of range", track.node)); }
        let kind = property_kind(&track.property);
        match (kind, track.node < n) {
            (None, _) => errors.push(format!("{ctx}: property {:?} not implemented", track.property)),
            (Some(_), true) => {
                let node = &scene.nodes[track.node];
                let ok = match track.property.as_str() {
                    "trim-start" | "trim-end" => matches!(node.content, Some(Content::Path(_))),
                    "fill.color" => matches!(&node.content, Some(Content::Path(p)) if p.fill.is_some()),
                    "stroke.color" => matches!(&node.content, Some(Content::Path(p)) if p.stroke.is_some()),
                    "text.fill" | "text.stroke" | "text.stroke-width" => matches!(&node.content, Some(Content::Paint { .. })),
                    "text.path-offset" => matches!(&node.content, Some(Content::Paint { text: Some(fx), .. }) if fx.path.is_some()),
                    p if p.starts_with("animators.") => animator_field_ok(node, p),
                    p if p.starts_with("glyph.") => matches!(node.content, Some(Content::Text(_))),
                    p if p.starts_with("effects.") => effect_field_ok(node, p),
                    "time.span" => node.time.is_some(),
                    "fluid.time" => matches!(node.content, Some(Content::Fluid(_))),
                    _ => true,
                };
                if !ok { errors.push(format!("{ctx}: property {:?} does not apply to node {} {:?}", track.property, track.node, node.name)); }
            }
            _ => {}
        }
        if track.stagger.is_some() && !track.property.starts_with("glyph.") { errors.push(format!("{ctx}: stagger only applies to glyph.* tracks")); }
        if let Some(st) = &track.stagger {
            if !st.each.is_finite() || !matches!(st.from.as_str(), "start" | "end" | "center") { errors.push(format!("{ctx}: stagger.each must be finite and from start|end|center")); }
        }
        if !track.duration.is_finite() || track.duration <= 0.0 { errors.push(format!("{ctx}: duration must be finite and positive")); }
        if !track.delay.is_finite() { errors.push(format!("{ctx}: delay must be finite")); }
        check_easing(&track.easing, &ctx, &mut errors);
        if track.keyframes.len() < 2 { errors.push(format!("{ctx}: at least two keyframes required")); }
        if track.keyframes.first().map(|k| k.offset) != Some(0.0) || track.keyframes.last().map(|k| k.offset) != Some(1.0) {
            errors.push(format!("{ctx}: first offset must be 0 and last offset 1"));
        }
        for (j, key) in track.keyframes.iter().enumerate() {
            let ctx = format!("{ctx} keyframe {j}");
            if !key.offset.is_finite() || !(0.0..=1.0).contains(&key.offset) { errors.push(format!("{ctx}: invalid offset")); }
            if let Some(e) = &key.easing { check_easing(e, &ctx, &mut errors); }
            let expect_transform = kind == Some(true);
            let is_color = color_property(&track.property);
            if kind.is_some() && !matches!((expect_transform, is_color, &key.value), (true, _, Value::Transform(_)) | (false, false, Value::Number(_)) | (false, true, Value::Color(_))) {
                errors.push(format!("{ctx}: value type does not match property"));
            }
            match &key.value {
                Value::Number(v) if !v.is_finite() => errors.push(format!("{ctx}: value must be finite")),
                Value::Color(c) if c.iter().any(|v| !v.is_finite()) => errors.push(format!("{ctx}: color must be finite")),
                Value::Transform(list) => check_transform(list, &ctx, &mut errors),
                _ => {}
            }
        }
        for pair in track.keyframes.windows(2) {
            if pair[0].offset >= pair[1].offset { errors.push(format!("{ctx}: offsets must be strictly increasing")); }
            if let (Value::Transform(a), Value::Transform(b)) = (&pair[0].value, &pair[1].value) {
                if !a.is_empty() && !b.is_empty() && (a.len() != b.len() || a.iter().zip(b).any(|(a,b)| std::mem::discriminant(a) != std::mem::discriminant(b))) {
                    errors.push(format!("{ctx}: adjacent transform lists are not interpolable"));
                }
            }
        }
    }
    let mut used = vec![false; scene.assets.len()];
    // Raster assets used as Image/Sequence content must match their rect * scale; image
    // brushes inside display lists are used at natural size.
    let sizes: Vec<_> = scene.assets.iter().enumerate().map(|(i, asset)| {
        if !asset.scale.is_finite() || asset.scale <= 0.0 { errors.push(format!("asset {i}: scale must be finite and positive")); }
        match png_size(&base_dir.join(&asset.path)) {
            Ok(size) => Some(size),
            Err(e) => { errors.push(format!("asset {i} {:?}: {e:#}", asset.path)); None }
        }
    }).collect();
    for (i, f) in scene.fonts.iter().enumerate() {
        if !base_dir.join(&f.path).is_file() { errors.push(format!("font {i} {:?}: file not found", f.path)); }
    }
    for (i, node) in scene.nodes.iter().enumerate() {
        let ctx = format!("node {i} {:?}", node.name);
        for (k, e) in node.effects.iter().enumerate() {
            let ctx = format!("{ctx} effect {k}");
            let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
            match e {
                Effect::Blur { radius } => if !finite(&[*radius]) || *radius < 0.0 { errors.push(format!("{ctx}: radius must be finite and >= 0")); },
                Effect::DropShadow { dx, dy, radius, color } => if !finite(&[*dx, *dy, *radius]) || *radius < 0.0 || !finite(color) { errors.push(format!("{ctx}: invalid drop-shadow")); },
                Effect::Glow { radius, color, strength } => if !finite(&[*radius, *strength]) || *radius < 0.0 || !finite(color) { errors.push(format!("{ctx}: invalid glow")); },
                Effect::Matte { source, mode } => {
                    if *source >= n || *source == i { errors.push(format!("{ctx}: matte source {source} invalid")); }
                    else if !scene.nodes[*source].hidden && !matches!(scene.nodes[*source].content, Some(Content::Fluid(_))) { warnings.push(format!("{ctx}: matte source {source} is not hidden (it is also drawn normally)")); }
                    if !matches!(mode.as_str(), "alpha" | "alpha-inverted" | "luma" | "luma-inverted") { errors.push(format!("{ctx}: matte mode {mode:?}")); }
                }
                Effect::Riso { inks, misreg, drift, drift_speed, grain, density, paper, .. } => {
                    if inks.is_empty() || inks.len() > 4 { errors.push(format!("{ctx}: riso needs 1..=4 inks")); }
                    if !inks.iter().all(|c| finite(c)) || !paper.map_or(true, |c| finite(&c)) || !finite(&[*misreg, *drift, *drift_speed, *grain, *density]) || *misreg < 0.0 || *misreg > 200.0 || drift.abs() > 200.0 { errors.push(format!("{ctx}: invalid riso (misreg/drift 0..200 px, finite values)")); }
                }
                Effect::Halftone { cell, angle, mode, ink, paper } => {
                    if !finite(&[*cell, *angle]) || *cell < 1.0 || *cell > 1000.0 || !finite(ink) || !paper.map_or(true, |c| finite(&c)) { errors.push(format!("{ctx}: invalid halftone (cell 1..1000 px)")); }
                    if !matches!(mode.as_str(), "mono" | "cmyk") { errors.push(format!("{ctx}: halftone mode {mode:?} (mono | cmyk)")); }
                }
                Effect::Grain { amount, size, .. } => if !finite(&[*amount, *size]) || *amount < 0.0 || *size < 0.25 || *size > 64.0 { errors.push(format!("{ctx}: invalid grain (amount >= 0, size 0.25..64)")); },
                Effect::Paper { texture, bleed, rough, color, .. } => if !finite(&[*texture, *bleed, *rough]) || !finite(color) || *bleed < 0.0 || *bleed > 100.0 { errors.push(format!("{ctx}: invalid paper (bleed 0..100 px)")); },
                Effect::Shadow { light, light_radius, height, color, strength } => if !finite(light) || !finite(&[*light_radius, *height, *strength]) || !finite(color) || *light_radius < 0.0 || *height < 0.0 || light[2] <= *height { errors.push(format!("{ctx}: invalid shadow (light z must exceed height, radius/height >= 0)")); },
                Effect::Sweep { pos, angle, width, color, strength } => if !finite(&[*pos, *angle, *width, *strength]) || !finite(color) || *width <= 0.0 { errors.push(format!("{ctx}: invalid sweep (width > 0)")); },
                Effect::Rays { light, length, color, strength } => if !finite(light) || !finite(&[*length, *strength]) || !finite(color) || *length < 0.0 || *length > 4.0 { errors.push(format!("{ctx}: invalid rays (length 0..4)")); },
                Effect::Rgbsplit { dx, dy, amount, mode } => {
                    if !finite(&[*dx, *dy, *amount]) || dx.abs() > 200.0 || dy.abs() > 200.0 || !(0.0..=1.0).contains(amount) || !matches!(mode.as_str(), "linear" | "radial") { errors.push(format!("{ctx}: invalid rgbsplit")); }
                }
                Effect::Glitch { amount, rate, block, .. } => {
                    if !finite(&[*amount, *rate, *block]) || !(0.0..=1.0).contains(amount) || !(0.0..=120.0).contains(rate) || !(1.0..=1024.0).contains(block) { errors.push(format!("{ctx}: invalid glitch")); }
                }
                Effect::Vhs { amount, .. } => if !amount.is_finite() || !(0.0..=1.0).contains(amount) { errors.push(format!("{ctx}: invalid vhs")); },
                Effect::Dither { levels, mode, size, palette } => {
                    if !levels.is_finite() || !(2.0..=256.0).contains(levels) || !matches!(mode.as_str(), "bayer" | "bluenoise" | "floyd") || !matches!(size, 2 | 4 | 8) || palette.as_ref().is_some_and(|p| !(2..=16).contains(&p.len()) || p.iter().any(|c| c.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) || c[3] != 1.0)) { errors.push(format!("{ctx}: invalid dither (opaque palette, 2..16 colors)")); }
                }
                Effect::Ascii { cell, charset, color, bg } => {
                    if !cell.is_finite() || !(6.0..=24.0).contains(cell) || charset.is_empty() || charset.chars().any(|c| !" .:-=+*#%@".contains(c)) || !matches!(color.as_str(), "mono" | "source") || bg.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { errors.push(format!("{ctx}: invalid ascii (charset from space .:-=+*#%@)")); }
                }
                Effect::Color { brightness, saturate, contrast, hue, tint, tint_amount, levels, lut } => {
                    if !finite(&[*brightness, *saturate, *contrast, *hue, *tint_amount]) || *brightness < 0.0 || *saturate < 0.0 || *contrast < 0.0 || !tint.map_or(true, |c| finite(&c)) { errors.push(format!("{ctx}: invalid color (brightness/saturate/contrast >= 0, finite values)")); }
                    if let Some(l) = levels {
                        if !finite(l) || l[1] <= l[0] || l[2] <= 0.0 { errors.push(format!("{ctx}: invalid levels (in_white > in_black, gamma > 0)")); }
                    }
                    if let Some(p) = lut {
                        if let Err(e) = crate::cube::Cube::load(&base_dir.join(p)) { errors.push(format!("{ctx}: lut {p:?}: {e}")); }
                    }
                }
            }
        }
        if let Some(mb) = &node.motion_blur {
            if mb.samples < 2 || mb.samples > 64 || !mb.shutter.is_finite() || mb.shutter <= 0.0 || mb.shutter > 4.0 { errors.push(format!("{ctx}: motion_blur needs 2..=64 samples and shutter in (0, 4]")); }
        }
        if let Some(w) = &node.world {
            if !w.tile.is_power_of_two() || w.tile < 64 || w.tile > 1024 || w.cache_mb < 16 || w.cache_mb > 16384 { errors.push(format!("{ctx}: world needs a power-of-two tile in 64..=1024 and cache_mb in 16..=16384")); }
            if node.time.is_some() || node.motion_blur.is_some() { errors.push(format!("{ctx}: world and time/motion_blur on the same node (put the world on a child)")); }
            let mut st = node.children.clone();
            while let Some(c) = st.pop() {
                if scene.nodes.get(c).is_some_and(|n| n.world.is_some()) { errors.push(format!("{ctx}: nested world (node {c}); one world per subtree")); }
                if let Some(n) = scene.nodes.get(c) { st.extend(n.children.iter().copied()); }
            }
        }
        if let Some(Content::Fluid(fl)) = &node.content {
            check_fluid(scene, i, fl, &ctx, &mut errors, &mut warnings);
        }
        if let Some(tf) = &node.time {
            check_time(scene, i, node, tf, &ctx, &mut errors, &mut warnings);
        }
        check_transform(&node.transform, &ctx, &mut errors);
        if !node.opacity.is_finite() || node.origin.iter().any(|v| !v.is_finite()) { errors.push(format!("{ctx}: opacity and origin must be finite")); }
        for clip in &node.clips { check_rrect(clip, &ctx, &mut errors); }
        if let Some(c) = &node.content { check_content(scene, c, &ctx, &sizes, &mut used, &mut errors, 0); }
    }
    for (i, used) in used.iter().enumerate() { if !used { warnings.push(format!("asset {i}: never used")); } }
    if errors.is_empty() { Ok(warnings) } else { Err(errors) }
}

/// Bytes of a fluid's per-frame dye cache (u8 per grid cell per frame).
pub fn fluid_cache_bytes(scene: &Scene, fl: &crate::FluidContent) -> u64 {
    let (gw, gh) = fluid_grid(fl);
    gw as u64 * gh as u64 * scene.frames.max(1) as u64
}

/// Simulation grid size of a fluid (cells).
pub fn fluid_grid(fl: &crate::FluidContent) -> (usize, usize) {
    let r = fl.res.clamp(0.05, 1.0);
    (((fl.rect[2] * r).round() as usize).max(8), ((fl.rect[3] * r).round() as usize).max(8))
}

/// Above this the fluid cache is large (warning).
pub const FLUID_CACHE_WARN: u64 = 512 << 20;

fn check_fluid(scene: &Scene, i: usize, fl: &crate::FluidContent, ctx: &str, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    use crate::FluidEmitter;
    let fin = |v: &[f64]| v.iter().all(|x| x.is_finite());
    if !fin(&fl.rect) || fl.rect[2] < 8.0 || fl.rect[3] < 8.0 { errors.push(format!("{ctx}: fluid rect must be finite and at least 8x8")); }
    if !fl.res.is_finite() || fl.res < 0.05 || fl.res > 1.0 { errors.push(format!("{ctx}: fluid res must be 0.05..=1 (grid cells per px)")); }
    if !fin(&[fl.viscosity, fl.vorticity, fl.buoyancy, fl.turbulence, fl.density, fl.shade, fl.time_offset]) || fl.viscosity < 0.0 || fl.vorticity < 0.0 || fl.turbulence < 0.0 || fl.density <= 0.0 { errors.push(format!("{ctx}: fluid viscosity/vorticity/turbulence must be >= 0, density > 0, all finite")); }
    if !(0.0..=1.0).contains(&fl.dissipation) || !(0.0..=1.0).contains(&fl.drag) { errors.push(format!("{ctx}: fluid dissipation and drag must be in [0, 1]")); }
    if !(0.0..1.0).contains(&fl.edge) { errors.push(format!("{ctx}: fluid edge must be in [0, 1)")); }
    if !(0.0..=1.0).contains(&fl.shade) { errors.push(format!("{ctx}: fluid shade must be in [0, 1]")); }
    if !(1..=20).contains(&fl.iterations) { errors.push(format!("{ctx}: fluid iterations (multigrid V-cycles) must be 1..=20")); }
    if !fin(&fl.color) || fl.color.iter().any(|c| !(0.0..=1.0).contains(c)) { errors.push(format!("{ctx}: fluid color components must be in [0, 1]")); }
    if node_is_fluid_ancestor(scene, i) { errors.push(format!("{ctx}: a fluid node cannot have children that are fluids")); }
    for (k, e) in fl.emitters.iter().enumerate() {
        let ec = format!("{ctx} emitter {k}");
        let (a, b) = e.window();
        if !fin(&[a, b]) || b < a { errors.push(format!("{ec}: start/end must be finite with end >= start")); }
        match e {
            FluidEmitter::Pour { at, dir, speed, radius, rate, wobble, .. } => if !fin(&[at[0], at[1], *dir, *speed, *radius, *rate, *wobble]) || *radius <= 0.0 || *rate < 0.0 { errors.push(format!("{ec}: pour needs radius > 0, rate >= 0, finite values")); },
            FluidEmitter::Stir { at, radius, strength, .. } => if !fin(&[at[0], at[1], *radius, *strength]) || *radius <= 0.0 { errors.push(format!("{ec}: stir needs radius > 0, finite values")); },
            FluidEmitter::Source { node, amount, speed, dir, noise, .. } => {
                if !fin(&[*amount, *speed, *dir, *noise]) || *amount < 0.0 { errors.push(format!("{ec}: source needs amount >= 0, finite values")); }
                if *node >= scene.nodes.len() || *node == i { errors.push(format!("{ec}: source node {node} invalid")); }
                else if subtree_has_fluid(scene, *node) { errors.push(format!("{ec}: source node {node} contains a fluid")); }
            }
        }
    }
    let bytes = fluid_cache_bytes(scene, fl);
    if bytes > FLUID_CACHE_WARN {
        let (gw, gh) = fluid_grid(fl);
        warnings.push(format!("{ctx}: fluid cache {gw}x{gh} cells x {} frames = {} MB (lower res or the scene length)", scene.frames, bytes >> 20));
    }
}

fn subtree_has_fluid(scene: &Scene, n: usize) -> bool {
    let mut st = vec![n];
    while let Some(x) = st.pop() {
        if matches!(scene.nodes[x].content, Some(Content::Fluid(_))) { return true; }
        st.extend(scene.nodes[x].children.iter().copied().filter(|&c| c < scene.nodes.len()));
    }
    false
}

fn node_is_fluid_ancestor(scene: &Scene, n: usize) -> bool {
    scene.nodes[n].children.iter().any(|&c| c < scene.nodes.len() && subtree_has_fluid(scene, c))
}

fn check_time(scene: &Scene, i: usize, node: &Node, tf: &crate::TimeFx, ctx: &str, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    use crate::{TimeFx, TimeMap};
    let span = tf.span();
    if !span.is_finite() || span.abs() > 30.0 { errors.push(format!("{ctx}: time span must be finite and within +-30 s")); }
    if node.motion_blur.is_some() { errors.push(format!("{ctx}: time effect and motion_blur on the same node (put one on a parent group)")); }
    match tf {
        TimeFx::Slitscan { axis, range, band, .. } => {
            if !matches!(axis.as_str(), "x" | "y") { errors.push(format!("{ctx}: slitscan axis must be x or y")); }
            if !band.is_finite() || *band < 1.0 || *band > 64.0 { errors.push(format!("{ctx}: slitscan band must be 1..=64 px")); }
            if let Some(r) = range { if !r.iter().all(|v| v.is_finite()) || (r[1] - r[0]).abs() < 1.0 { errors.push(format!("{ctx}: slitscan range must be [a, b] with |b - a| >= 1 (b < a: reversed)")); } }
        }
        TimeFx::Echo { count, decay, tint, .. } => {
            if *count < 1 || *count > 32 { errors.push(format!("{ctx}: echo count must be 1..=32")); }
            if !decay.is_finite() || *decay <= 0.0 || *decay > 1.0 { errors.push(format!("{ctx}: echo decay must be in (0, 1]")); }
            if tint.len() > 2 || tint.iter().flatten().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { errors.push(format!("{ctx}: echo tint takes at most 2 colours with components in [0, 1]")); }
        }
        TimeFx::Displace { map, levels, .. } => {
            if *levels < 2 || *levels > 64 { errors.push(format!("{ctx}: displace levels must be 2..=64")); }
            match map {
                TimeMap::Radial { center, radius } => if !center.iter().all(|v| v.is_finite()) || !radius.is_finite() || *radius <= 0.0 { errors.push(format!("{ctx}: radial map needs a finite center and radius > 0")); },
                TimeMap::Linear { from, to } => if !from.iter().chain(to).all(|v| v.is_finite()) || (from[0] - to[0]).hypot(from[1] - to[1]) < 1.0 { errors.push(format!("{ctx}: linear map needs from/to at least 1 px apart")); },
                TimeMap::Noise { scale, octaves, .. } => if !scale.is_finite() || *scale < 1.0 || *octaves < 1 || *octaves > 8 { errors.push(format!("{ctx}: noise map needs scale >= 1 px and 1..=8 octaves")); },
                TimeMap::Layer { node: m } => {
                    if *m >= scene.nodes.len() || *m == i { errors.push(format!("{ctx}: layer map node {m} invalid")); }
                    else if !scene.nodes[*m].hidden { warnings.push(format!("{ctx}: layer map node {m} is not hidden (it is also drawn normally)")); }
                }
            }
        }
    }
}

/// Some(true) = transform-valued, Some(false) = number-valued, None = unknown property.
pub fn property_kind(p: &str) -> Option<bool> {
    match p {
        "transform" | "glyph.transform" => Some(true),
        "opacity" | "glyph.opacity" | "trim-start" | "trim-end" => Some(false),
        p if p.starts_with("effects.") => Some(false),
        "time.span" | "fluid.time" => Some(false),
        "fill.color" | "stroke.color" => Some(false),
        "text.fill" | "text.stroke" | "text.stroke-width" | "text.path-offset" => Some(false),
        p if p.starts_with("animators.") => Some(false),
        _ => None,
    }
}

/// Track properties whose values are colors (`{"color": [r, g, b, a]}`).
pub fn color_property(p: &str) -> bool {
    matches!(p, "fill.color" | "stroke.color" | "text.fill" | "text.stroke")
}

fn animator_field_ok(node: &Node, p: &str) -> bool {
    let mut it = p.split('.');
    let (Some("animators"), Some(i), Some(field), None) = (it.next(), it.next(), it.next(), it.next()) else { return false };
    let Ok(i) = i.parse::<usize>() else { return false };
    let Some(Content::Paint { text: Some(fx), .. }) = &node.content else { return false };
    match (fx.animators.get(i).map(|a| &a.selector), field) {
        (Some(TextSelector::Range { .. }), "start" | "end" | "offset" | "amount") => true,
        (Some(TextSelector::Keys { .. }), "amount") => true,
        _ => false,
    }
}

fn check_text_fx(fx: &TextFx, ctx: &str, errors: &mut Vec<String>) {
    let fin = |v: f64| v.is_finite();
    for (i, a) in fx.animators.iter().enumerate() {
        let ctx = format!("{ctx} animator {i}");
        if !matches!(a.by.as_str(), "char" | "word" | "line") { errors.push(format!("{ctx}: by must be char|word|line")); }
        let nums = [a.opacity, a.rotate, a.blur].into_iter().flatten().chain(a.translate.into_iter().flatten()).chain(a.scale.into_iter().flatten()).chain(a.color.into_iter().flatten());
        if nums.into_iter().any(|v| !fin(v)) { errors.push(format!("{ctx}: values must be finite")); }
        if a.blur.is_some_and(|b| b < 0.0) || a.opacity.is_some_and(|o| !(0.0..=1.0).contains(&o)) { errors.push(format!("{ctx}: blur >= 0 and opacity 0..1")); }
        if a.color.is_some_and(|c| c.iter().any(|v| !(0.0..=1.0).contains(v))) { errors.push(format!("{ctx}: color components must be in [0,1]")); }
        match &a.selector {
            TextSelector::Range { start, end, offset, shape, amount, order, .. } => {
                if ![*start, *end, *offset, *amount].iter().all(|v| fin(*v)) { errors.push(format!("{ctx}: range values must be finite")); }
                if !matches!(shape.as_str(), "square" | "ramp-up" | "ramp-down" | "triangle" | "round" | "smooth") { errors.push(format!("{ctx}: shape {shape:?} (square|ramp-up|ramp-down|triangle|round|smooth)")); }
                if !matches!(order.as_str(), "forward" | "reverse" | "random") { errors.push(format!("{ctx}: order must be forward|reverse|random")); }
            }
            TextSelector::Keys { units, easing, amount } => {
                check_easing(easing, &ctx, errors);
                if units.is_empty() || units.iter().any(|u| u.is_empty()) { errors.push(format!("{ctx}: keys need at least one key per unit")); }
                if !fin(*amount) || units.iter().flatten().any(|k| !fin(k[0]) || !fin(k[1])) { errors.push(format!("{ctx}: keys must be finite")); }
                if units.iter().any(|u| u.windows(2).any(|w| w[1][0] < w[0][0])) { errors.push(format!("{ctx}: key times must be sorted")); }
            }
        }
    }
    if let Some(p) = &fx.path {
        if !p.offset.is_finite() { errors.push(format!("{ctx}: path offset must be finite")); }
        if !p.d.trim_start().starts_with(['M', 'm']) { errors.push(format!("{ctx}: path d must start with M")); }
    }
}

fn effect_field_ok(node: &Node, p: &str) -> bool {
    let mut it = p.split('.');
    let (Some("effects"), Some(i), Some(field), None) = (it.next(), it.next(), it.next(), it.next()) else { return false };
    let Ok(i) = i.parse::<usize>() else { return false };
    match (node.effects.get(i), field) {
        (Some(Effect::Blur { .. }), "radius") => true,
        (Some(Effect::DropShadow { .. }), "radius" | "dx" | "dy") => true,
        (Some(Effect::Glow { .. }), "radius" | "strength") => true,
        (Some(e), f) if e.is_look() => e.look_fields().contains(&f),
        _ => false,
    }
}

fn check_content(scene: &Scene, c: &Content, ctx: &str, sizes: &[Option<(u32, u32)>], used: &mut [bool], errors: &mut Vec<String>, depth: usize) {
    let mut uses = Vec::new();
    match c {
        Content::Image { asset, rect, variants } => { check_rect(rect, ctx, errors); uses.push((*asset, *rect)); for v in variants { uses.push((*v, *rect)); } }
        Content::Sequence { rect, frames } => {
            check_rect(rect, ctx, errors);
            if frames.first().map(|f| f.0) != Some(0) { errors.push(format!("{ctx}: sequence must start at frame 0")); }
            if frames.windows(2).any(|p| p[0].0 >= p[1].0) { errors.push(format!("{ctx}: sequence frames must be strictly sorted")); }
            for &(frame, asset) in frames {
                if frame >= scene.frames { errors.push(format!("{ctx}: sequence frame {frame} out of range")); }
                uses.push((asset, *rect));
            }
        }
        Content::Rect { rect, color } => {
            check_rrect(rect, ctx, errors);
            if color.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { errors.push(format!("{ctx}: color components must be in [0,1]")); }
        }
        Content::Paint { rect, items, text, .. } => {
            check_rect(rect, ctx, errors);
            if let Some(fx) = text {
                check_text_fx(fx, ctx, errors);
            }
            let mut depth = 0i32;
            for d in items {
                match d {
                    Draw::Push { .. } => depth += 1,
                    Draw::Pop => { depth -= 1; if depth < 0 { errors.push(format!("{ctx}: pop without push")); return; } }
                    Draw::Glyphs { font, .. } if *font >= scene.fonts.len() => errors.push(format!("{ctx}: font {font} out of range")),
                    Draw::Glyphs { stroke: Some(st), .. } if !(st.width.is_finite() && st.width >= 0.0) || !matches!(st.join.as_str(), "round" | "miter" | "bevel") => errors.push(format!("{ctx}: glyph stroke width must be >= 0, join round|miter|bevel")),
                    _ => {}
                }
                let brush = match d { Draw::Fill { brush, .. } | Draw::Stroke { brush, .. } | Draw::Glyphs { brush, .. } => Some(brush), _ => None };
                if let Some(Brush::Image { asset, .. }) = brush {
                    if *asset >= scene.assets.len() { errors.push(format!("{ctx}: image asset {asset} out of range")); } else { used[*asset] = true; }
                }
            }
            if depth != 0 { errors.push(format!("{ctx}: unbalanced push/pop")); }
        }
        Content::Switch { rect, frames, states } => {
            check_rect(rect, ctx, errors);
            if depth > 0 { errors.push(format!("{ctx}: nested switch")); }
            if frames.first().map(|f| f.0) != Some(0) { errors.push(format!("{ctx}: switch must start at frame 0")); }
            if frames.windows(2).any(|p| p[0].0 >= p[1].0) { errors.push(format!("{ctx}: switch frames must be strictly sorted")); }
            for &(_, s) in frames { if s >= states.len() { errors.push(format!("{ctx}: switch state {s} out of range")); } }
            for (k, st) in states.iter().enumerate() { check_content(scene, st, &format!("{ctx} state {k}"), sizes, used, errors, depth + 1); }
        }
        Content::Text(t) => {
            if t.font >= scene.fonts.len() { errors.push(format!("{ctx}: font {} out of range", t.font)); }
            if !(t.size.is_finite() && t.size > 0.0) { errors.push(format!("{ctx}: text size must be positive")); }
            if !matches!(t.align.as_str(), "left" | "center" | "right") { errors.push(format!("{ctx}: align must be left|center|right")); }
            if t.align != "left" && t.max_width.is_none() { errors.push(format!("{ctx}: align {} needs max_width", t.align)); }
            if !matches!(t.unit.as_str(), "glyph" | "word" | "line") { errors.push(format!("{ctx}: unit must be glyph|word|line")); }
        }
        Content::Path(p) => {
            if p.fill.is_none() && p.stroke.is_none() { errors.push(format!("{ctx}: path needs fill or stroke")); }
            if p.d.trim().is_empty() { errors.push(format!("{ctx}: empty path")); }
            if p.fill_transform.is_some_and(|m| m.iter().any(|v| !v.is_finite())) { errors.push(format!("{ctx}: fill_transform must be finite")); }
            if !(0.0..=1.0).contains(&p.trim[0]) || !(0.0..=1.0).contains(&p.trim[1]) { errors.push(format!("{ctx}: trim must be within [0,1]")); }
        }
        Content::Cloud(c) => check_cloud(scene, c, ctx, errors),
        Content::Fluid(_) => {} // checked per node (needs the node index and warnings)
    }
    for (id, rect) in uses {
        if id >= scene.assets.len() { errors.push(format!("{ctx}: asset {id} out of range")); continue; }
        used[id] = true;
        if let Some((w,h)) = sizes[id] {
            let scale = scene.assets[id].scale;
            let expected = ((rect[2] * scale).round(), (rect[3] * scale).round());
            if expected != (w as f64, h as f64) { errors.push(format!("{ctx}: asset {id} PNG size {w}x{h} != expected {}x{}", expected.0, expected.1)); }
        }
    }
}

fn check_cloud(scene: &Scene, c: &CloudContent, ctx: &str, errors: &mut Vec<String>) {
    if c.count == 0 || c.count > 4_000_000 { errors.push(format!("{ctx}: cloud count must be 1..=4000000")); }
    if !(c.radius.is_finite() && c.radius > 0.0) { errors.push(format!("{ctx}: cloud radius must be positive")); }
    if !(0.0..=1.0).contains(&c.radius_jitter) { errors.push(format!("{ctx}: radius_jitter must be within [0,1]")); }
    if c.palette.is_empty() { errors.push(format!("{ctx}: cloud palette is empty")); }
    if c.palette.iter().flatten().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { errors.push(format!("{ctx}: palette components must be in [0,1]")); }
    if c.weights.len() > c.palette.len() || c.weights.iter().any(|w| !w.is_finite() || *w < 0.0) { errors.push(format!("{ctx}: weights: at most one nonnegative weight per palette entry")); }
    if c.forms.is_empty() { errors.push(format!("{ctx}: cloud needs at least one form")); }
    if c.forms.windows(2).any(|w| w[1].at < w[0].at) { errors.push(format!("{ctx}: forms must be sorted by `at`")); }
    if c.projection.windows(2).any(|w| w[1].t < w[0].t) || c.projection.iter().any(|v| v.m.iter().any(|x| !x.is_finite())) { errors.push(format!("{ctx}: projection keys must be finite and sorted by t")); }
    for (k, f) in c.forms.iter().enumerate() {
        let fc = format!("{ctx} form {k}");
        if ![f.at, f.dur, f.stagger, f.size, f.opacity].iter().all(|v| v.is_finite()) || f.dur < 0.0 || f.size < 0.0 { errors.push(format!("{fc}: at/dur/stagger/size/opacity must be finite, dur and size >= 0")); }
        if !matches!(f.stagger_by.as_str(), "index" | "random" | "distance" | "x" | "y" | "angle") { errors.push(format!("{fc}: stagger_by must be index|random|distance|x|y|angle")); }
        if !matches!(f.assign.as_str(), "curve" | "angle" | "index" | "random") { errors.push(format!("{fc}: assign must be curve|angle|index|random")); }
        check_easing(&f.easing, &fc, errors);
        if let Some(d) = &f.drift {
            if !(d.amp.is_finite() && d.scale.is_finite() && d.scale > 0.0 && d.speed.is_finite()) { errors.push(format!("{fc}: drift needs finite amp/speed and scale > 0")); }
        }
        if let Some(col) = &f.color {
            if !matches!(col.by.as_str(), "random" | "index" | "x" | "y" | "z" | "distance" | "angle" | "dir") { errors.push(format!("{fc}: color.by must be random|index|x|y|z|distance|angle|dir")); }
            if col.split.len() > c.palette.len() || col.split.iter().any(|w| !w.is_finite() || *w < 0.0) { errors.push(format!("{fc}: color.split: at most one nonnegative fraction per palette entry")); }
        }
        match &f.shape {
            CloudShape::Field { rect, .. } | CloudShape::Grid { rect, .. } => check_rect(rect, &fc, errors),
            CloudShape::Terrain { rect, scale, .. } => { check_rect(rect, &fc, errors); if *scale <= 0.0 { errors.push(format!("{fc}: terrain scale must be positive")); } }
            CloudShape::Spiral { radius, arms, .. } => if *radius <= 0.0 || *arms == 0 { errors.push(format!("{fc}: spiral needs radius > 0 and arms >= 1")); },
            CloudShape::Sphere { radius, .. } => if *radius <= 0.0 { errors.push(format!("{fc}: sphere radius must be positive")); },
            CloudShape::Gaussian { sigma, .. } => if sigma.iter().any(|s| !s.is_finite() || *s < 0.0) { errors.push(format!("{fc}: gaussian sigma must be >= 0")); },
            CloudShape::Text(t) => {
                if t.font >= scene.fonts.len() { errors.push(format!("{fc}: font {} out of range", t.font)); }
                if !(t.size.is_finite() && t.size > 0.0) { errors.push(format!("{fc}: text size must be positive")); }
                if t.text.trim().is_empty() { errors.push(format!("{fc}: empty text")); }
                if !matches!(t.align.as_str(), "left" | "center" | "right") || (t.align != "left" && t.max_width.is_none()) { errors.push(format!("{fc}: align must be left|center|right (center/right need max_width)")); }
            }
            CloudShape::Points { points } => if points.is_empty() || points.iter().any(|p| !(2..=3).contains(&p.len()) || p.iter().any(|v| !v.is_finite())) { errors.push(format!("{fc}: points must be a non-empty list of [x, y] or [x, y, z]")); },
        }
    }
}

fn check_easing(e: &Easing, ctx: &str, errors: &mut Vec<String>) {
    let problem = match *e {
        Easing::Linear => None,
        Easing::Steps(n, pos) if n == 0 || (n == 1 && pos == StepPos::JumpNone) => Some("steps count must be >= 1 (jump-none requires >= 2)"),
        Easing::CubicBezier(x1,y1,x2,y2) if ![x1,y1,x2,y2].iter().all(|v| v.is_finite()) || !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) => Some("cubic-bezier parameters must be finite and x1/x2 in [0,1]"),
        Easing::Spring { mass, stiffness, damping, velocity } if ![mass,stiffness,damping,velocity].iter().all(|v| v.is_finite()) || mass <= 0.0 || stiffness <= 0.0 || damping < 0.0 => Some("spring parameters must be finite, mass/stiffness > 0 and damping >= 0"),
        Easing::Spring { damping: 0.0, .. } => Some("spring with zero damping never settles; settling-time easing requires damping > 0"),
        _ => None,
    };
    if let Some(problem) = problem { errors.push(format!("{ctx}: {problem}")); }
}
fn check_rect(r: &[f64;4], ctx: &str, errors: &mut Vec<String>) {
    if r.iter().any(|v| !v.is_finite()) || r[2] <= 0.0 || r[3] <= 0.0 { errors.push(format!("{ctx}: rect must be finite with positive size")); }
}
fn check_rrect(r: &RRect, ctx: &str, errors: &mut Vec<String>) {
    check_rect(&r.rect, ctx, errors);
    if r.radius.iter().any(|v| !v.is_finite() || *v < 0.0) { errors.push(format!("{ctx}: rect radii must be finite and nonnegative")); }
}
fn check_transform(list: &[TransformFn], ctx: &str, errors: &mut Vec<String>) {
    for t in list {
        let finite = match *t { TransformFn::Translate(a,b) | TransformFn::Scale(a,b) => a.is_finite() && b.is_finite(), TransformFn::Rotate(a) => a.is_finite() };
        if !finite { errors.push(format!("{ctx}: transform must be finite")); }
    }
}
fn png_size(path: &Path) -> anyhow::Result<(u32,u32)> {
    let mut header = [0u8; 33];
    std::fs::File::open(path)?.read_exact(&mut header)?;
    anyhow::ensure!(&header[..8] == b"\x89PNG\r\n\x1a\n" && header[8..12] == 13u32.to_be_bytes() && &header[12..16] == b"IHDR", "invalid PNG signature or IHDR");
    let w = u32::from_be_bytes(header[16..20].try_into()?);
    let h = u32::from_be_bytes(header[20..24].try_into()?);
    anyhow::ensure!(w > 0 && h > 0, "PNG dimensions must be positive");
    Ok((w,h))
}
