//! The capability catalog: every kind/verb/channel/feature with its level, pipeline stage and
//! S0 status. Source of the SPEC tables (`agentfx catalog --md`) and of the JSON Schema (`agentfx schema`).
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub struct Item {
    pub name: &'static str,
    /// doc | story | motion | raw
    pub level: &'static str,
    pub group: &'static str,
    /// composite | paint | layout | per-frame | audio | -
    pub stage: &'static str,
    /// impl | partial | planned
    pub status: &'static str,
    pub syntax: &'static str,
    pub doc: &'static str,
}

const fn it(name: &'static str, level: &'static str, group: &'static str, stage: &'static str, status: &'static str, syntax: &'static str, doc: &'static str) -> Item {
    Item { name, level, group, stage, status, syntax, doc }
}

pub const ITEMS: &[Item] = &[
    // ---- document
    it("video", "doc", "document", "-", "impl", "video \"Title\" size=1920x1080 fps=30 voice=<tts voice> rate=+6% theme=studio-dark captions=plain|karaoke|off formats=[16:9, 9:16] lead=2f tail=6f", "Header. One per file."),
    it("theme", "doc", "document", "-", "impl", "theme studio-dark\n  colors accent=#17845a ...\n  type h1=92 min=20\n  motion base=0.5s pop=spring(260,20)\n  layout safe=6% radius=18\n  lint min-contrast=3 no-emdash=true\n  bg linear(...)", "Brand tokens; referenced as $name."),
    it("pronounce", "doc", "document", "-", "impl", "pronounce \"staging.example.test\" \"staging dot example dot test\"", "Spoken substitution; captions keep the written form."),
    it("music", "doc", "audio", "audio", "impl", "music src=bed.mp3 vol=0.25 duck=0.08", "Music bed, auto-ducked under every say."),
    it("audio reactivity", "motion", "audio", "composite", "impl", "music src=track.wav bpm=128\n  rect #b origin=bottom scale-y=audio(band=3, gain=1.5, smooth=80ms) ; opacity=audio(level)\n  pulse #ring on=beat|bar amount=0.15 ; at beat:32 / at bar:8", "Offline music analysis (level, onsets, 8 bands, beat grid) lowered to linear keyframe tracks; beat:N / bar:N anchors."),
    it("visualizer", "story", "audio", "composite", "impl", "visualizer #v type=bars|radial|circle|waveform count=32 w= h= gap= radius= thick= colors=[..] mirror gain= smooth=", "Music visualizer: a layer of rects with per-frame tracks."),
    it("chapterbar", "doc", "component", "composite", "impl", "chapterbar label=\"...\" scenes=[a, b, c]", "Progress bar for scenes with title=."),
    it("when", "doc", "format", "-", "impl", "when 9:16 | portrait | landscape | square\n  set #id k=v\n  <elements>", "Per-format overrides and format-only elements."),
    it("scene", "doc", "timing", "-", "impl", "scene <id> title=\"...\" in=fade|push|slide|wipe|iris|zoom|magic|cut tdur=0.6s pre=0.5s post=0.5s dur=<override> min= bg=<paint>", "A shot. Length = narration unless dur= overrides."),
    it("say", "story", "timing", "audio", "impl", "say \"Text with [[anchor]] and [[two words|name]].\" min=5s pre= post= spoken=\"...\"", "Voice-over beat; TTS word timings drive all anchors."),
    it("pause", "story", "timing", "-", "impl", "pause 0.8s", "Silent beat."),
    it("@anchor", "story", "timing", "-", "impl", "@Reiter verb #id ... ; verb ...\n@Reiter.end+0.2s  @2.5s  @start  @end-1s  @prev+0.3s", "Event time: word start/end, scene time, previous event; verbs follow."),
    // ---- story elements
    it("text", "story", "text", "layout", "impl", "text #id \"**bold** {accent} `code`\" style=display|h1|h2|h3|body|small|label size= color= weight= align= upper split=word|char", "Rich text. h1/h2/h3/body/small/label are shorthands."),
    it("split (kinetic type)", "motion", "text", "composite", "impl", "text #t \"...\" split=word|char in=rise stagger=60ms", "Per-word/per-glyph animation."),
    it("split=line", "motion", "text", "layout", "impl", "text #t \"...\" w=760 split=line in=rise stagger=180ms", "One layer per laid-out line (native line breaks, S1)."),
    it("variable font axes", "motion", "text", "layout", "impl", "text ... axes=\"wdth 80, GRAD 50\" ; anim #t wght 0s:300 1s:900 wdth 0s:75 1s:125 GRAD 0s:0 1s:100", "Static axes on any variable font; animated wght/font-weight, wdth, opsz, slnt and up to 4 custom (uppercase) axes per video, relaid out per frame (repaint states)."),
    it("custom font file", "doc", "text", "layout", "impl", "font \"Brand\" src=\"fonts/Brand.woff2\" ; text ... font=Brand", "TTF/OTF/TTC/WOFF/WOFF2 registered under the alias (direct mode decodes WOFF/WOFF2; Chrome reference gets @font-face). E_FONT, W_FONT_FAMILY."),
    it("opentype features", "story", "text", "layout", "impl", "text ... features=\"ss01 off, tnum, cv05 2\"", "font-feature-settings on the text (replaces the stage default 'cv11','ss01')."),
    it("text stroke", "story", "text", "paint", "impl", "text ... stroke=#000 sw=6 [stroke-over]", "Real outline from the glyph outlines (reel-ir glyph stroke): sw = visible width below the fill, or centered over it with stroke-over. Contrast lint counts it."),
    it("text color track", "motion", "text", "paint", "impl", "anim #t color 0s:#fff 1s:$accent", "Plain text (no accent/code runs, no matte, no child layers): one reel-ir text.fill track instead of repaint states."),
    it("text animators", "motion", "text", "paint", "impl", "animate #t by=char|word|line [range=0%..100%] [start= end= offset=-100%..100% amount=] shape=square|ramp-up|ramp-down|triangle|round|smooth order=forward|reverse|random seed= | times=[0s,0.3s,..] dir=in|out ; dur= ease= opacity= x= y= scale= sx= sy= rot= blur= color=", "After Effects range selectors on one text node (renderer evaluates per unit, per frame), or per-unit key times. Parameters given as a..b animate over dur=."),
    it("text on path", "motion", "text", "paint", "impl", "path #p d=\"M...\" ; text #t \"...\" on=#p offset=40 ; anim #t offset 0s:0 2s:600", "Glyphs ride the path (rest geometry, scene space), rotated to its tangent; offset px animates (path-offset channel); glyphs off the ends hide."),
    it("auto-fit", "story", "text", "layout", "impl", "text ... w=800 h=200 fit", "Shrinks font to fit (load-time layout hook)."),
    it("captions", "story", "text", "paint", "impl", "video captions=plain|karaoke", "Captions from VO words; karaoke highlights the spoken word; anchors in mint."),
    it("caption pack", "story", "text", "composite", "impl", "captions style=karaoke|pop|pill|hormozi|typewriter|rise|outline|safe [src=talk.srt|talk.vtt|talk.json audio=talk.wav offset=] font= size= weight= color= hi= key= box= stroke= sw= shadow= place=bottom|center|top y= w= maxchars= lines= maxwords= upper keywords=[..] gap= minword= mincue=", "Word-by-word social captions from narration or a transcript (SRT/VTT/Whisper JSON); phrase grouping, balanced lines with real font metrics, per-format safe areas, read-time lint. Header `video captions=<style>` applies a style to every narrated scene."),
    it("title", "story", "component", "layout", "impl", "title #t \"Main\" sub=\"Subline\" kicker=\"LABEL\"", "Title block; svg/icon/image children go left of the title."),
    it("stack/row/col/grid", "story", "layout", "layout", "impl", "row gap=24 align= justify= wrap ; grid cols=3 ; stack dir=auto", "Flex/grid layout. dir=auto = row in landscape, col in portrait."),
    it("box", "story", "layout", "layout", "impl", "box #b pad=30 ...", "Plain surface container."),
    it("card", "story", "component", "layout", "impl", "card #c \"Title\" sub=\"Body\" icon=bolt kicker=\"NEW\" variant=surface|glass|accent|outline", "Designed card: icon tile, kicker, title, body. Cards in a row share its width and height."),
    it("stat", "story", "component", "layout", "impl", "stat #s value=12480 from=0 label=\"Nutzer\" prefix= suffix=\" %\" delta=\"+12 %\" sub=\"seit Juli\" icon=user spark=[3, 5, 8] in=count", "KPI tile: label, counting number, delta pill, sparkline. `count #s to=` works on the tile."),
    it("note", "story", "component", "layout", "impl", "note #n \"Text\" title=\"Heads up\" kind=info|tip|warn|danger|success icon=", "Callout box with accent bar and tinted icon."),
    it("node", "story", "component", "layout", "impl", "node #a \"Label\" sub=\"Detail\" icon=db variant=default|primary|muted status=ok|warn|err", "Diagram node with icon tile and status dot (use in `diagram` + `link`)."),
    it("steps", "story", "component", "layout", "impl", "steps #p \"Plan\" \"Build\" \"Ship\" active=2", "Numbered steps with connectors; parts #p-s0.. stagger on enter."),
    it("list", "story", "component", "layout", "impl", "list #l \"Eins\" \"Zwei\" icon=check", "Icon list; items stagger on enter."),
    it("pill", "story", "component", "paint", "impl", "pill \"Neu\" color=$accent2", "Badge."),
    it("quote", "story", "component", "layout", "impl", "quote #q \"Text\" by=\"Name, Rolle\"", "Pull quote."),
    it("window", "story", "mock", "layout", "impl", "window #w title=\"Dashboard\" w=900 h=520\n  stat ... ; note ...", "App window mock whose body holds elements."),
    it("device", "story", "mock", "composite", "impl", "device #d type=laptop|tablet|phone shot=app.png", "Device frame around a screenshot (no hotspots; use browser/phone for those)."),
    it("design", "doc", "document", "-", "impl", "design lens=swiss|editorial|keynote|flat|blueprint ; design src=design.reel ; design \"Name\" base=light|dark ⏎   colors ... ⏎   type display=\"Family\" body=\"Family\" base=40 ratio=1.333 ⏎   surface style=elevated|flat|outline|glass|tinted ⏎   skin node=tile|step|tag", "Design spec (L2): extends the theme tokens; see design/AUTHORING.md."),
    it("progress", "story", "component", "layout", "impl", "progress #p n=6 value=0 label=\"Schritt\"", "Segmented progress with a value / n counter; `advance #p to=3` fills it."),
    it("panel", "story", "component", "layout", "impl", "panel #d kicker=\"Details\" ⏎   pane #a title=\"Agent\" ⏎     item \"lint\" chip=\"ok\" status=ok|run|wait|fail icon=", "Detail panel: one pane at a time; `page #d to=a` swaps panes."),
    it("skins", "story", "component", "layout", "impl", "node #a \"Chat\" sub=\"..\" n=01 skin=tile|step|tag", "HTML/CSS skins for nodes (design/skins), drawn with the design tokens; parts active/done."),
    it("place / x,y", "story", "layout", "layout", "impl", "place=top-left|bottom|... x=40% y=120 anchor=center", "Absolute placement (place uses the safe area)."),
    it("image", "raw", "media", "paint", "impl", "image src=a.png w= h= fit=cover", "Bitmap layer. Direct mode reads PNG only (other formats: W_IR_UNSUPPORTED)."),
    it("video", "raw", "media", "per-frame", "partial", "video src=clip.mp4", "HTML backend only (seeked per frame); direct mode does not draw video yet (native decode planned)."),
    it("svg", "raw", "media", "paint", "impl", "svg w=120 h=120 |\n  <svg ...>...</svg>", "Inline SVG escape hatch. With w=/h= the root <svg> is scaled into the box (viewBox fit, its preserveAspectRatio; a viewBox is derived from width/height when missing)."),
    it("html", "raw", "media", "layout", "impl", "html #b w=900 |\n  <style>.k{display:grid}</style>\n  <div class=\"k\">...</div>", "Raw HTML + CSS block; laid out and painted natively by Blitz in direct mode (static CSS). data-part=\"x\" makes an inner element the layer #block.x for verbs."),
    it("lottie", "raw", "media", "per-frame", "planned", "lottie src=anim.json", "Planned."),
    it("shader", "raw", "media", "per-frame", "planned", "shader wgsl=fx.wgsl", "Planned (native renderer)."),
    it("icon", "story", "component", "paint", "impl", "icon name=sync size=48 color=$mint", "Built-in stroke icons."),
    it("browser / phone", "story", "mock", "composite", "impl", "browser #app url=... shot=a.png hotspots=a.json w=1600 crop=[x, y, w, h]\n  hot #extra 10 20 300 40\n  toast #t \"Saved\" in=rise at=@x", "Screenshot in a window/device; hotspot ids become verb targets."),
    it("terminal", "story", "mock", "paint", "impl", "terminal #t in=type |\n  $ cargo build\n  Finished", "Typed commands, then output."),
    it("code", "story", "generator", "layout", "impl", "code #c lang=ts diff |\n  const a = 1\n- const b = 2\n+ const b = 3", "Highlighted code; `diff #c` animates -/+ lines."),
    it("chart", "story", "generator", "composite", "impl", "chart #c type=bar|line|area|pie|donut data=[..] labels=[..] unit=% max= highlight=4 grid=false axis=false values", "Chart with y axis, gridlines, tick and value labels, legend (donut: largest slice in the center); `draw #c` or in=draw. Parts: #c.b1 (bar), #c.d1 (point), #c.s1 (slice)."),
    it("counter", "story", "generator", "layout", "impl", "counter #n from=0 to=1280 prefix= suffix=\" users\"", "Integer counter; count #n to=."),
    it("cloud (point cloud)", "motion", "generator", "per-frame", "impl", "cloud #swarm n=20000 seed=7 r=2.4 jitter=0.25 palette=[paper, #E8442E] weights=[0.64, 0.36] persp= mblur=8 ⏎   form 0s field flow=[60, -10] noise=40 ⏎   form 1.2s text \"3000\" weight=800 size=60% dur=1s stagger=0.6s by=x ease=spring(150,15) color=dir angle=30 split=[0.64, 0.36] ⏎   form 4s spiral r=40% arms=3 spin=25 ⏎   form 7s terrain z=on amp=80 rows=120 ⏎   form 9s cluster sx=20% noise=10 ; sphere ; grid ; points", "Instanced point cloud: N points (up to millions) morph between formations with per-point stagger, springs, seeded drift and color rules; one layer, evaluated per frame by the renderer (reel-ir `cloud`). 3D forms go through the scene camera. Direct mode only."),
    it("fluid", "motion", "generator", "per-frame", "impl", "fluid #ink w=100% h=100% res=1/4 viscosity=0.0005 dissipation=0.99 drag=0.995 vorticity=12 buoyancy=0 turbulence=0 seed=3 color=#E8442E density=2.5 shade=0.3 ⏎ @0.2s pour #ink at=30%,80% dir=-80deg rate=3 ⏎ stir #ink at=50%,50% strength=5 ⏎ @1s ink #title into #ink ⏎ reveal #shot with #ink", "2D stable-fluids layer: dye advected by a simulated flow (vorticity confinement, pressure projection) on a reduced grid, upsampled. Simulated once by the renderer and cached per frame (deterministic, frame n = f(n)); rewinds and time maps replay the cache. Direct mode only."),
    it("particles", "motion", "generator", "per-frame", "impl", "particles #p count=60 seed=3 x=50% y=40% speed=900 spread=360 gravity=1400 life=1.4s shape=confetti|dot|square colors=[..] emit=@word", "Seeded particle burst (exact projectile paths)."),
    it("lower3", "story", "component", "composite", "impl", "lower3 #l \"Name\" sub=\"Role\" icon=user in=left", "Lower third (glass panel, accent bar)."),
    it("toast", "story", "component", "composite", "impl", "toast #t \"Saved\" in=rise at=@word", "Notification pill."),
    it("chapter", "story", "component", "composite", "impl", "chapter \"Panels\" n=1 for=1.2s", "Full-frame chapter card at scene start."),
    it("link (connector)", "story", "generator", "paint", "impl", "link #l #a -> #b curve dashed", "Connector between two element ids (load-time layout), draw-on."),
    it("diagram", "story", "generator", "layout", "partial", "diagram dir=row gap=90\n  node #a \"Input\"\n  node #b \"Model\"\nlink #a -> #b", "Row/col auto layout of nodes; graph layout planned."),
    it("maps", "story", "generator", "paint", "planned", "map ...", "Planned."),
    // ---- shapes / paint
    it("rect/ellipse/circle", "motion", "shape", "paint", "impl", "rect #r w=300 h=200 radius=24 fill=$accent stroke=#fff sw=4", "Primitive shapes."),
    it("poly/star", "motion", "shape", "paint", "impl", "star #s points=5 inner=0.45 size=200", "Polygon and star paths."),
    it("path", "motion", "shape", "paint", "impl", "path #p d=\"M0 0 c ...\" vb=24x24 w=400 h=200 fill= stroke=$mint|linear(...) sw=6 trim=0 ; anim #p stroke 0s:#fff 1s:$mint fill ...", "Bezier path (full SVG path data: absolute and relative M L H V C S Q T A Z). Gradient strokes; animated fill/stroke colors are reel-ir color tracks (no repaint)."),
    it("trim paths", "motion", "shape", "paint", "impl", "draw #p ; anim #p trim 0s:0 1s:1 ; trim-start", "Draw-on strokes."),
    it("dashes", "motion", "shape", "paint", "impl", "dash=\"10 8\"", "Dashed strokes."),
    it("boolean ops", "motion", "shape", "paint", "planned", "union/subtract", "Planned (native)."),
    it("path morphing", "motion", "shape", "paint", "planned", "anim #p d ...", "Planned (native; HTML would need JS)."),
    it("SVG import", "motion", "shape", "paint", "partial", "svg src=icon.svg", "Embedded as raw layer; per-path addressing planned."),
    it("solid / linear / radial / conic", "motion", "paint", "paint", "impl", "fill=#hex | $token | linear(155deg, #a 0%, #b 100%) | radial(at 30% 40%, ...) | conic(0deg, ...)", "Paints."),
    it("mesh gradient", "motion", "paint", "paint", "impl", "fill=mesh(2x2, #a, #b, #c, #d)", "Grid of colors, bicubic in OKLab; rastered once into a cached image both backends draw (static). Box, path, text and mask fills. Literal colors (theme tokens resolve first)."),
    it("image fill / pattern", "motion", "paint", "paint", "partial", "fill=image(tex.png cover|contain|fill|none)", "Image fills on boxes, paths, text and masks (direct mode: PNG only; contain/none on paths and text pad the edges). Patterns planned."),
    it("animated gradient", "motion", "paint", "paint", "impl", "anim #bg grad-angle 0s:120deg 6s:240deg grad0 0s:#a 3s:#b", "Angle + up to 4 stop colors animate."),
    // ---- compositing
    it("groups / precomps", "motion", "compositing", "composite", "impl", "layer #g x= y= w= h=\n  ...", "Nested layers; transforms compose."),
    it("alpha / luma masks", "motion", "compositing", "paint", "impl", "mask=linear(90deg, #000 60%, transparent) mask-mode=luma ; anim #x mask-x ...", "Paint/image masks. mask-x 1 -> 0 slides a 3x-wide mask across the layer (put the soft edge between 33% and 66%)."),
    it("track mattes (layer as matte)", "motion", "compositing", "paint", "partial", "text #t \"BIG\" fill=image(a.png) | fill=linear(...) | fill=mesh(...)", "Text matte via text fill; arbitrary layer mattes planned."),
    it("blend modes", "motion", "compositing", "composite", "impl", "blend=normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity|plus", "Layer blend mode (W3C compositing; plus = add, also `add`)."),
    it("clip / wipe / iris reveal", "motion", "compositing", "paint", "impl", "enter #x wipe|iris ; clip=box", "Animated clip shapes."),
    // ---- effects
    it("gaussian blur", "motion", "effects", "paint", "impl", "blur=8 ; anim #x blur 0s:20 1s:0", ""),
    it("directional / radial blur", "motion", "effects", "paint", "planned", "", "Planned (native)."),
    it("motion blur", "motion", "effects", "per-frame", "impl", "mblur=16 shutter=180deg ; anim #x x 0s:0 0.3s:900 mblur=24 shutter=1f", "Sub-frame shutter (reel-ir motion_blur): samples 2..64, shutter angle (180deg = half a frame, default) or frames, max 4f. Direct mode only; the HTML backend ignores it."),
    it("drop / inner shadow", "motion", "effects", "paint", "impl", "shadow=sm|md|lg|inner|\"0 20 60 #0008\"", ""),
    it("glow", "motion", "effects", "paint", "impl", "glow=24 glow-color=$mint ; glow #x", ""),
    it("color adjust", "motion", "effects", "paint", "impl", "brightness=1.2 saturate=0 contrast=1.4 hue=90deg ; anim #x saturate 0s:0 1s:1", "CSS filter semantics in both backends (direct: reel-ir `color` effect, animatable)."),
    it("tint / levels / LUT", "motion", "effects", "paint", "impl", "tint=#F2C14E tint-amount=0.8 ; levels=0.05,0.95,1.1[,0,1] ; lut=grade.cube", "Tint maps luma to a color, levels remap black/white/gamma, 3D .cube LUTs (trilinear). Direct mode only; the HTML backend ignores them."),
    it("grain / vignette", "motion", "effects", "paint", "impl", "grain=0.06 grain-mode=static|live grain-size=1 ; vignette=0.6", "Seeded grain inside the layer (reel-ir grain). static (default) costs nothing at rest; live changes every frame and defeats frame skipping. HTML: animated overlay."),
    it("risograph", "motion", "effects", "paint", "impl", "riso=[#FF48B0, #0078BF, #FFE800] misreg=3 riso-drift=1.5 riso-grain=0.35 riso-density=0.2 riso-paper=$bg seed=4 ; anim #x misreg 0s:12 1s:2", "Layer (or scene) split into 1-4 spot-ink plates, seeded misregistration per plate (slow drift: per-frame), multiplied, grain and density variation in the ink. Direct mode only."),
    it("rgbsplit", "motion", "effects", "paint", "impl", "rgbsplit=1 rgbsplit-dx=7 rgbsplit-dy=0 rgbsplit-mode=linear|radial", "RGB channel offsets. Amount and dx/dy are animatable. Direct mode only."),
    it("glitch", "motion", "effects", "paint", "impl", "glitch=0.5 glitch-rate=6 glitch-block=22 seed=42", "Seeded digital displacement. Amount, rate and block are animatable. Direct mode only."),
    it("vhs", "motion", "effects", "paint", "impl", "vhs=0.5 seed=42", "Animated tape noise, scanlines and chroma bleed. Animatable amount. Direct mode only."),
    it("dither", "motion", "effects", "paint", "impl", "dither=3 dither-mode=bayer|bluenoise|floyd dither-size=4 dither-palette=[#000, #fff]", "Quantization with optional opaque palette. Animatable levels. Direct mode only."),
    it("ascii", "motion", "effects", "paint", "impl", "ascii=8 ascii-color=source|mono ascii-bg=#000", "Bitmap character filter. Animatable cell size, custom ascii-charset. Direct mode only."),
    it("halftone", "motion", "effects", "paint", "impl", "halftone=10 halftone-angle=45deg halftone-mode=mono|cmyk halftone-ink=#111 halftone-paper=#fff ; anim #x halftone 0s:160 4s:7", "AM halftone dots (area from darkness) on a rotated grid; cmyk = four screens. Animatable cell. Direct mode only."),
    it("paper", "motion", "effects", "paint", "impl", "paper=0.6 bleed=1.5 rough=0.5 paper-color=#F6F1E7", "Ink bleed (blur-threshold), rough edges, paper fibers and tooth. Seeded, static. Direct mode only."),
    it("light source", "motion", "effects", "composite", "impl", "light #sun x=20% y=-10% z=900 radius=60 ; anim #sun x 0s:0 6s:1400", "Invisible point/sphere light in the scene world (z = height above the page, radius = penumbra). Animate it like any element."),
    it("soft shadow (SDF)", "motion", "effects", "paint", "impl", "shadow=soft(#sun, height=40, opacity=0.5, color=#000)", "Shadow of the layer as a sheet `height` px above the page, penumbra from its signed distance field; a moving light sweeps it. Direct mode only."),
    it("light sweep", "motion", "effects", "paint", "impl", "sweep #card dur=1.4s angle=20deg width=25% color=#FFF1D6 strength=0.35", "A soft band of light crosses the layer once (channel sweep 0..1). Direct mode only."),
    it("god rays", "motion", "effects", "paint", "impl", "rays=#sun rays-length=0.6 rays-strength=1 rays-color=#FFE9C4 ; anim #plate rays 0s:0 1s:1.4", "Light behind the layer streams through its holes (knocked-out type), radial blur away from the light. Direct mode only."),
    it("chromatic aberration / displacement", "motion", "effects", "paint", "planned", "", "Planned."),
    it("depth of field", "motion", "space", "paint", "planned", "camera focus=0 dof=0.02", "Planned (static blur by z works via blur=)."),
    // ---- motion
    it("keyframes", "motion", "motion", "-", "impl", "anim #x <channel> <t>:<v>[~ease] ... [ease=] [repeat=3] [yoyo]", "Explicit keyframes; times relative to the event, or @anchor:value."),
    it("bezier easing", "motion", "motion", "-", "impl", "ease=out-cubic | cubic(.2,.8,.2,1) | steps(4) | hold", ""),
    it("spring / bounce", "motion", "motion", "-", "impl", "ease=spring(170,26) | bouncy | snappy | bounce(3)", "Physical spring kept symbolic in the IR; HTML samples it into CSS linear()."),
    it("stagger / repeat", "motion", "motion", "-", "impl", "in=rise stagger=80ms ; anim ... repeat=3 yoyo", ""),
    it("seeded random", "motion", "motion", "-", "impl", "seed=7", "SplitMix64; same seed, same frames."),
    it("expressions", "motion", "motion", "-", "partial", "anim #x rot wiggle(2, 5deg, 1) dur=3s ; sin(freq, amp)", "Pure functions of time, sampled to keys; general expressions planned."),
    it("time remap", "motion", "motion", "-", "impl", "rewind #g speed=4 dur=0.5s [then=play|hold] ; timemap #g 0s:0s 0.3s:0s 0.9s:-2.4s~in-cubic [then=hold] ; #scene", "A subtree's local time as a function of its parent's: reverse, freeze, slow/fast, scrub. Baked per frame by the compiler (exact at frames for every ease, split text, trim, particles, repeats)."),
    it("time effects", "motion", "effects", "per-frame", "impl", "slitscan #g axis=y span=1.2s band=4 ; echo #x n=6 dt=60ms decay=0.6 blend=add colors=$red,$blue ; timedisplace #grid map=radial|linear|noise|#layer span=0.8s levels=24 ; freeze #x for=1s", "The subtree rendered at other times (reel-ir `time`): slit-scan rows/columns from earlier times, decaying echo copies, per-pixel time displacement. span/dt animate with later verbs (`slitscan #g span=0 dur=0.2s`). Direct mode only. freeze = hold remap."),
    it("motion paths + auto-orient", "motion", "motion", "composite", "planned", "anim #x path=\"M...\" orient", "Planned."),
    it("magic move", "story", "transitions", "composite", "impl", "scene b in=magic   (same #id in both scenes)", "Shared elements morph position/size between scenes."),
    // ---- space
    it("2.5D camera / parallax", "motion", "space", "composite", "partial", "camera persp=1200 ; layer z=-400 ; anim #camera x 0s:0 6s:300 ; dolly z=200 ; orbit roty=12", "Perspective camera; depth gives parallax. Direct mode linearizes each 3D layer per frame (rotated planes stay parallelograms, W_IR_APPROX)."),
    it("world canvas + camera director", "story", "space", "composite", "impl", "world #w 16000x9000 ⏎   place #p at 7000,4000 scale=0.01 rot=-3 ; camera tour #a -> #b ease=spring(170,24) dwell=narration ; camera zoom-through #a -> #b dur=5s ; camera follow #x fit=0.7 lead=120ms ; camera focus #k overlay=#000a", "Boards on one large canvas (nested scale = powers of ten); the camera frames ids on smooth zoom-pan paths (exact at 1e4+ zoom). The renderer draws the static world from a mip tile cache while moving, vectors at rest. Direct mode."),
    it("nest + camera loop (infinite zoom)", "story", "space", "composite", "impl", "nest #t levels=28 scale=0.72 rot=4 cycle=[#FF2BD6, #22E4FF, #FFC23D] w=1920 h=1920 ⏎   rect x=50 y=50 w=1820 h=1820 radius=150 stroke=$cycle sw=14 ; camera loop #t levels=6 dur=6s", "Self-similar nest (copy i scaled k^i, turned r*i, hue cycle) and a seamless loop: constant log-speed zoom of exactly N levels per loop, frame N equals frame 0. Lint checks the seam. Direct mode."),
    // ---- stylised 3D (scene3d)
    it("scene3d", "motion", "3d", "per-frame", "impl", "scene3d #w fov=40 camera=(0, 2, 8) target=(0, 0, 0) roll=0 near=0.1 far=200 [ortho=8 | iso] light=(-3, 5, 4) light-color= light-intensity=0.75 ambient=0.4 ambient-color= fog=#05010D fog-near=10 fog-far=30 [fog-density=] bg= aa=2 bloom=14 bloom-strength=1 shadow=0.45|off shadow-blur=6 shadow-color= outline=2 outline-color= material=toon color=", "A 3D scene rasterised by the CPU renderer into one layer of the 2D pipeline (looks, effects, blend, masks, motion blur apply to it). y up, world units free. A static scene is drawn once and cached; camera or object tracks re-render it. Direct mode only."),
    it("3D objects", "motion", "3d", "per-frame", "impl", "box #b w=2 h=1.4 d=2 bevel=0.1 ; cube size=1 ; plane w= h= ; floor y=-1 [size=200] ; sphere r=1 seg=48 ; cylinder r=0.5 h=2 r2= ; cone r= h= ; torus r=1 tube=0.3 ; group #g ⏎   x= y= z= | pos=(x, y, z) rx= ry= rz= scale= sx= sy= sz= pivot=(x, y, z) shadow=off ground", "Children of a scene3d (nest under a group or any object: transforms compose). Rotations in degrees, yaw applied last. floor = a plane facing up that receives shadows."),
    it("3D extruded text / path", "motion", "3d", "per-frame", "impl", "extrude #L \"L\" size=1.6 depth=0.6 font=Inter weight=800 tracking=0 ; extrude #logo d=\"M0 0 L10 0 L5 8 Z\" size=2 depth=0.3", "Glyph outlines from the text stack (or SVG path data, nonzero fill) triangulated and extruded, centered on the ink box; size = font size (text) or height (path) in world units."),
    it("3D bevel on extrusions", "motion", "3d", "per-frame", "planned", "extrude ... bevel=0.05", "Planned; boxes have bevel= (rounded edges)."),
    it("3D mesh (OBJ)", "motion", "3d", "per-frame", "partial", "mesh #m src=model.obj size=2", "Wavefront OBJ: v, vn, vt, f (polygons fanned), normalized to size. No MTL materials, no glTF yet (planned)."),
    it("3D screen", "motion", "3d", "per-frame", "impl", "layer #ui x=0 y=0 w=1920 h=1080 ⏎   ... ; scene3d ⏎   screen #crt w=4 h=2.25 src=#ui [res=1] emissive=0.2", "A plane whose texture is a 2D layer of the scene (hidden in the 2D pass), rendered every frame it changes (mip-mapped, trilinear): UI mockups, code, charts and kinetic type on a monitor or stage screen."),
    it("3D materials", "motion", "3d", "per-frame", "impl", "material=flat|lambert|toon|phong|wire color= opacity= emissive=0.6 bands=3 specular=0.5 shininess=32 outline=2 outline-color= crease=35 wire=1.5 wire-color= double-sided texture=wood.png", "Toon = banded light; outline = screen-space silhouette and crease lines from the depth/normal buffer; wire = feature edges as lines; emissive drives the scene's bloom. Texture: PNG on box faces / planes (no UV editing)."),
    it("3D lights", "motion", "3d", "per-frame", "impl", "scene3d light=(-3, 5, 4) ambient=0.4 ; light #lamp point=(0, 4, 3) color=#7CFF6B intensity=0.5 range=6 ; light #rim dir=(4, 2, -3)", "Directional key light + hemispheric ambient on the scene line, extra point/directional lights as children (x/y/z and intensity animatable)."),
    it("3D ground shadows", "motion", "3d", "per-frame", "partial", "floor ... ; scene3d shadow=0.45 shadow-blur=6 shadow-color=#000", "Planar: casters projected onto receiving planes (floor, ground) along the first light, softened by a blur. No self-shadowing or shadows on other objects (shadow maps planned)."),
    it("3D transparency", "motion", "3d", "per-frame", "partial", "box ... opacity=0.5", "Triangles sorted back to front per frame (no order-independent transparency); transparent objects get no outline."),
    it("3D GPU path", "motion", "3d", "per-frame", "planned", "", "Planned (wgpu); the CPU rasteriser is the reference."),
    // ---- transitions
    it("transitions", "story", "transitions", "composite", "impl", "scene x in=cut|fade|push|push-up|slide|wipe|wipe-right|iris|zoom|magic tdur=0.6s", "Zoom-through blurs the outgoing scene."),
    it("custom mask transitions", "story", "transitions", "paint", "planned", "in=mask(shape.svg)", "Planned."),
    // ---- layout
    it("safe areas", "story", "layout", "layout", "impl", "theme layout safe=6%", "Content flows inside; lint flags anything outside."),
    it("multi-format", "story", "layout", "layout", "impl", "video formats=[16:9, 9:16, 1:1, 4:5] ; agentfx html --format 9:16 ; when portrait ...", "One source, several aspect ratios."),
    it("constraints solver", "story", "layout", "layout", "planned", "", "Explicit constraints between ids planned; flex/grid now."),
    // ---- audio
    it("TTS word timings", "story", "audio", "audio", "impl", "video voice=en_US-ljspeech-medium rate=+6%", "piper (default, local; words spread over the audio and snapped to pauses), edge-tts WordBoundary (opt-in) or estimated (`--tts none`); cached per sentence hash."),
    it("SFX on events", "story", "audio", "audio", "impl", "@x click #b sfx ; sfx click|pop|whoosh|file.wav", "Built-in synthesized sfx or files."),
];

pub struct VerbDoc {
    pub name: &'static str,
    pub syntax: &'static str,
    pub doc: &'static str,
}
pub const VERB_DOCS: &[VerbDoc] = &[
    VerbDoc { name: "enter", syntax: "enter #id fade|rise|drop|left|right|pop|zoom|blur|wipe|iris|draw|type|flip|count dur= ease= stagger=", doc: "Make an element appear (also as element prop in=preset at=@anchor)." },
    VerbDoc { name: "exit", syntax: "exit #id fade|rise|drop|left|right|pop|zoom|blur|wipe dur=", doc: "Make it disappear (prop out=preset until=@anchor)." },
    VerbDoc { name: "show / hide", syntax: "show #id ; hide #id", doc: "Instant." },
    VerbDoc { name: "activate / deactivate", syntax: "activate #node for=2s ; deactivate #node", doc: "Accent state of a node/card/stat (gradient, glow, slight pop): mark the active step of a diagram while it is spoken." },
    VerbDoc { name: "done / undone", syntax: "done #node ; undone #node", doc: "Check badge on a node (native or skin part `done`); ends its active state." },
    VerbDoc { name: "advance", syntax: "advance #progress to=3", doc: "Fill a progress component up to step 3 and count." },
    VerbDoc { name: "page", syntax: "page #panel to=<pane id>", doc: "Swap a detail panel to another pane (blur swap)." },
    VerbDoc { name: "emph", syntax: "emph #id pulse|shake|glow|bounce|wobble|flash ; pulse #id ; glow #id for=1.5s", doc: "Emphasis. On a hotspot: expanding ring." },
    VerbDoc { name: "highlight", syntax: "highlight #hotspot for=1.6s color=$accent2", doc: "Ring + tint around a screenshot hotspot." },
    VerbDoc { name: "spotlight", syntax: "spotlight #hotspot for=2s", doc: "Dim everything but the hotspot." },
    VerbDoc { name: "callout", syntax: "callout #hotspot \"Label\" side=right|left|top|bottom for=2s", doc: "Label next to a hotspot." },
    VerbDoc { name: "zoom / pan", syntax: "zoom #hotspot z=1.9 dur=1s ; zoom #element ; zoom out ; zoom x=0.7 y=0.3 z=2 ; pan #hotspot", doc: "Camera inside a screenshot (clamped), or the scene camera onto any laid-out element (centered, z fits it)." },
    VerbDoc { name: "cursor", syntax: "cursor -> #hotspot dur=0.6s dx= dy=", doc: "Move the cursor (starts at the anchor)." },
    VerbDoc { name: "click / dblclick", syntax: "click #hotspot sfx", doc: "Cursor arrives AT the anchor, presses, ripple." },
    VerbDoc { name: "drag", syntax: "drag #hotspot dx=240 dy=0 dur=1.6s", doc: "Press, drag, release." },
    VerbDoc { name: "type", syntax: "type #hotspot \"text\" ; type #textElement", doc: "Types text into a hotspot field or reveals a text element." },
    VerbDoc { name: "swap", syntax: "swap #app shot=next.png [hotspots=next.json] via=fade|cut dur=0.2s", doc: "Next UI state (screenshot) with its own hotspots." },
    VerbDoc { name: "count", syntax: "count #n to=1280 dur=1.2s", doc: "Animate a counter." },
    VerbDoc { name: "draw", syntax: "draw #path|#chart|#link dur=1.2s", doc: "Trim-path draw-on / chart build." },
    VerbDoc { name: "diff", syntax: "diff #code", doc: "Removed lines flash red and collapse, added lines expand green." },
    VerbDoc { name: "burst", syntax: "burst #particles", doc: "Emit the particle burst." },
    VerbDoc { name: "move", syntax: "move #id x=120 y=-40 rot=10 scale=1.2 dur= ease= ; move #a to=#b", doc: "Relative move (px); to=#b centers it on another element's box (x/y add on top)." },
    VerbDoc { name: "set", syntax: "set #id opacity=0.4 color=$mint", doc: "Instant channel change." },
    VerbDoc { name: "anim", syntax: "anim #id scale 0s:0.8 0.6s:1~spring(170,20) opacity 0s:0 0.3s:1", doc: "Motion-level keyframes (several channels per line)." },
    VerbDoc { name: "rewind", syntax: "rewind #g speed=4 dur=0.5s then=play|hold ; rewind #scene ...", doc: "Play the subtree's local time backwards at `speed`x for `dur`, then play on (default) or hold. speed=0 freezes." },
    VerbDoc { name: "timemap", syntax: "timemap #g 0s:0s 0.5s:-2s~in-quad 1s:-2s then=play|hold", doc: "Piecewise local time: <parent offset>:<local offset> keys relative to the event, eases between keys." },
    VerbDoc { name: "slitscan", syntax: "slitscan #g axis=y|x span=1.2s band=4 dir=down|up|right|left [dur= ease=]", doc: "Each row (column) of the group shows it span*(position in its box) earlier. Repeat with a new span (and dur=) to animate it; span=0 = sharp." },
    VerbDoc { name: "echo", syntax: "echo #x n=6 dt=60ms decay=0.6 blend=normal|add|multiply|screen colors=#f00,#00f", doc: "n trailing copies at t - k*dt, opacity decay^k, tinted newest -> oldest. dt animates like span." },
    VerbDoc { name: "timedisplace", syntax: "timedisplace #g map=radial|linear|noise|#layer span=0.8s levels=24 center=0.5,0.5 angle=0 scale=160 seed=1", doc: "Per-pixel delay from a map (0 = now, 1 = span ago), quantized to levels; a #layer map is hidden and read as luma." },
    VerbDoc { name: "freeze", syntax: "freeze #x [for=1s]", doc: "The subtree's time stops (the rest plays on); with for= it resumes from where it stopped." },
    VerbDoc { name: "pour", syntax: "pour #ink at=30%,80% dir=-80deg speed=700 radius=2.5% rate=3 wobble=14 dur=1s", doc: "A jet of dye into a fluid: velocity pulled to speed along dir (0 = right, -90 = up), rate = dye per second at its center." },
    VerbDoc { name: "stir", syntax: "stir #ink at=50%,50% radius=18% strength=4 dur=0.8s", doc: "A vortex in a fluid: rigid rotation of strength rad/s inside a Gaussian of radius (negative = counter-clockwise)." },
    VerbDoc { name: "ink", syntax: "ink #title into #ink dur=0.5s amount=1.6 noise=420 speed=0 dir=-90deg [keep]", doc: "The layer's shape becomes dye of the fluid (it fades out while it pours in, unless keep) with a curl-noise kick: type dissolves into ink or smoke." },
    VerbDoc { name: "reveal", syntax: "reveal #shot with #ink [mode=alpha|alpha-inverted] [only]", doc: "The layer shows only where the fluid's dye is (a matte); only = the fluid itself is not drawn." },
    VerbDoc { name: "animate", syntax: "animate #title by=char|word|line shape=ramp-up range=0%..25% offset=-25%..100% dur=1.6s ease=linear opacity=0 y=60 blur=10 ; animate #cap by=word times=[0s, 0.3s, 0.8s] dur=0.3s y=30 opacity=0", doc: "Text animator (AE range selector or per-unit key times) on one text node: the selected units get the props scaled by their amount." },
    VerbDoc { name: "sweep", syntax: "sweep #card dur=1.4s angle=20deg width=25% color=#FFF1D6 strength=0.35 ease=", doc: "A soft band of light crosses the layer (look `sweep`)." },
    VerbDoc { name: "camera tour / zoom-through / follow / focus", syntax: "camera tour #a -> #b -> #c ease=spring(170,24) dwell=narration|1.5s move=1.2s fit=0.85 ; camera zoom-through #a -> #b dur=5s ; camera follow #x fit=0.7 lead=120ms for=3s ; camera focus #k overlay=#000a pad=28 for=1.6s", doc: "Camera director on world canvases: frames ids (safe area, fit), van Wijk zoom-pan paths, dark-overlay focus. Lint: E_CAMERA_TARGET, E_CAMERA_TINY, W_CAMERA_FAST." },
    VerbDoc { name: "camera loop", syntax: "camera loop #nest levels=6 dur=6s dir=in|out zoom=1 wrap=on|off", doc: "Seamless infinite zoom into a `nest`: constant log-speed, exactly `levels` levels per `dur`, the nest turns back by its rot, then wraps. Lint: W_LOOP_SEAM, W_LOOP_CONTENT, W_LOOP_SELF_SIMILAR." },
    VerbDoc { name: "dolly / orbit", syntax: "dolly z=300 dur=3s ; orbit roty=15 ; orbit #world by=30deg pitch=10deg dur=4s ease= ; dolly #world by=2|to=5 dur=", doc: "2.5D camera moves; on a scene3d id: the 3D camera circles its target (yaw, pitch) or moves toward it (by= forward, to= distance), baked per frame." },
    VerbDoc { name: "truck / pedestal / lookat", syntax: "truck #world by=1.5 dur=2s ; pedestal #world by=1 ; lookat #world #crate dur=1s ; lookat #world at=(0, 1, 0)", doc: "3D camera: slide sideways / up with the target, or aim at an object or point." },
    VerbDoc { name: "camera3d", syntax: "camera3d #world orbit=30deg pitch= dolly=2 truck= pedestal= lookat=#obj fov=30 roll=5 dur=4s ease=inout-cubic", doc: "Several 3D camera moves in one eased move." },
    VerbDoc { name: "spin / tumble", syntax: "spin #crate axis=y by=360deg|turns=1 dur=1.2s ease= ; tumble #die speed=120 dur=2s", doc: "3D object: turn about an axis; tumble about all three." },
    VerbDoc { name: "drop (3D)", syntax: "drop #crate from=3 bounce=0.35 g=25 ; box ... in=drop at=@1s from=5", doc: "On a 3D object: falls from `from` above its height and bounces (baked per frame); on 2D layers the physics drop." },
    VerbDoc { name: "assemble / explode", syntax: "assemble #group dist=4 spin=160 stagger=70ms dur=0.9s seed=1 ; explode #group", doc: "Pieces (the group's children, or the object itself) fly in from seeded offsets and turns, or fly apart and fade." },
    VerbDoc { name: "stack", syntax: "@1s stack #ship axis=y gap=0.04 from=3 stagger=180ms dur=0.45s", doc: "Piles a group's objects along an axis from the group's origin, each falling in from `from` above (containers on a ship). Write it as an event (`@t`)." },
    VerbDoc { name: "3D presets", syntax: "box ... in=fade|pop|rise|drop|spin|flip|assemble out=fade|pop|sink|spin|explode", doc: "Entrances and exits of 3D objects." },
    VerbDoc { name: "sfx", syntax: "sfx click|pop|whoosh|file.wav gain=0.6", doc: "Sound effect at the event." },
    VerbDoc { name: "focus / play", syntax: "focus #id ; play #video", doc: "Planned." },
];

pub fn markdown() -> String {
    let mut s = String::new();
    let mut groups: Vec<&str> = vec![];
    for i in ITEMS {
        if !groups.contains(&i.group) {
            groups.push(i.group);
        }
    }
    let st = |x: &str| match x {
        "impl" => "implemented",
        "partial" => "partial",
        _ => "planned",
    };
    for g in groups {
        s.push_str(&format!("\n#### {}\n\n| Feature | Level | Stage | Status | Syntax | Notes |\n|---|---|---|---|---|---|\n", g));
        for i in ITEMS.iter().filter(|i| i.group == g) {
            s.push_str(&format!("| {} | {} | {} | {} | `{}` | {} |\n", i.name, i.level, i.stage, st(i.status), i.syntax.replace('\n', " ⏎ ").replace('|', "\\|"), i.doc));
        }
    }
    s.push_str("\n#### Verbs (story level; compile into motion tracks)\n\n| Verb | Syntax | Meaning |\n|---|---|---|\n");
    for v in VERB_DOCS {
        s.push_str(&format!("| {} | `{}` | {} |\n", v.name, v.syntax.replace('|', "\\|"), v.doc));
    }
    s.push_str("\n#### Channels (motion level)\n\n| Channel | Stage |\n|---|---|\n");
    for n in crate::ir::Ch::NAMES {
        let c = crate::ir::Ch::parse(n).unwrap();
        s.push_str(&format!("| {} | {:?} |\n", n, c.stage()));
    }
    s
}

pub fn json() -> Value {
    json!({
        "features": ITEMS.iter().map(|i| json!({"name": i.name, "level": i.level, "group": i.group, "stage": i.stage, "status": i.status, "syntax": i.syntax, "doc": i.doc})).collect::<Vec<_>>(),
        "verbs": VERB_DOCS.iter().map(|v| json!({"name": v.name, "syntax": v.syntax, "doc": v.doc})).collect::<Vec<_>>(),
        "elements": crate::elements::ELEMENTS.to_vec(),
        "channels": crate::ir::Ch::NAMES.to_vec(),
        "enter": crate::events::ENTER.to_vec(),
        "emph": crate::events::EMPH.to_vec(),
    })
}

/// JSON Schema (draft 2020-12) of the JSON form.
pub fn schema() -> Value {
    let mut kinds: Vec<&str> = crate::elements::ELEMENTS.to_vec();
    kinds.extend_from_slice(crate::lower::VERBS);
    kinds.extend_from_slice(&["video", "theme", "colors", "type", "motion", "layout", "lint", "bg", "pronounce", "music", "visualizer", "chapterbar", "when", "scene", "say", "pause", "at", "hot", "hide", "show", "emph", "form", "font"]);
    kinds.extend_from_slice(crate::s3d::OBJ_KINDS);
    kinds.push("stack");
    kinds.sort();
    kinds.dedup();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://agentfx.dev/schema/reel-script-0.json",
        "title": "Reel Script v0 (JSON form)",
        "description": "1:1 JSON form of the Reel Script text format. Every node is {kind, args, props, body, children}. Value spellings are the same as in text form (\"0.4s\", \"#17845a\", \"$accent\", \"@Reiter+0.2s\"). Typing happens against the catalog (`agentfx catalog`).",
        "type": "object",
        "required": ["nodes"],
        "properties": {
            "reel": {"const": "0"},
            "nodes": {"type": "array", "items": {"$ref": "#/$defs/node"}}
        },
        "$defs": {
            "value": {"oneOf": [
                {"type": "number"}, {"type": "string"}, {"type": "boolean"},
                {"type": "array", "items": {"$ref": "#/$defs/value"}}
            ]},
            "node": {
                "type": "object",
                "required": ["kind"],
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "description": "element, verb or document node; `at` = event anchor (args[0] = anchor expression)", "examples": kinds},
                    "args": {"type": "array", "items": {"$ref": "#/$defs/value"}},
                    "props": {"type": "object", "additionalProperties": {"$ref": "#/$defs/value"}},
                    "body": {"type": "string"},
                    "children": {"type": "array", "items": {"$ref": "#/$defs/node"}}
                }
            }
        }
    })
}
