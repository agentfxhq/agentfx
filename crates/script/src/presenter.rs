//! Compiler-only vector presenter. All poses lower to ordinary layers and tracks.
use crate::{ast::Node, ir::*, lower::{Ctx, Ev, id_arg}, values::{Ease, Paint, ColorRef, Rgba, suggest}};

const SKINS: &[&str] = &["flat", "line", "paper"];
const MOODS: &[&str] = &["neutral", "happy", "serious", "surprised"];
const GESTURES: &[&str] = &["point", "wave", "nod", "shrug", "think", "present", "count"];
const SWAY: f64 = 2.2;
const EASE: Ease = Ease::Cubic(0.33, 1.0, 0.68, 1.0);
#[derive(Clone)]
pub struct Rig {
    root: usize, scene: usize, source: Node, scale: f64,
    torso: usize, head: usize, eyes: usize, pupils: usize, brows: [usize; 4],
    rest: [usize; 4], mouth: [usize; 8], arms: [[usize; 3]; 2], fingers: [[usize; 5]; 2],
}
fn prop(n: &Node, k: &str, d: &str) -> String { n.props.get(k).map(|v| v.raw()).unwrap_or(d.into()) }
fn random(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (*seed >> 11) as f64 / ((1u64 << 53) as f64)
}
/// Orthographic German/English approximation, intentionally not a phoneme recognizer.
pub fn visemes(word: &str) -> Vec<usize> {
    let text = word.to_lowercase();
    let mut s = text.as_str();
    let mut out = vec![];
    while !s.is_empty() {
        let pair = [("sch", 4), ("ch", 4), ("sh", 4), ("th", 7), ("ph", 6), ("ee", 2), ("ie", 2), ("oo", 4), ("ei", 1), ("ai", 1), ("au", 3), ("eu", 3), ("äu", 3)];
        if let Some((g, v)) = pair.iter().find(|(g, _)| s.starts_with(g)) {
            out.push(*v); s = &s[g.len()..]; continue;
        }
        let c = s.chars().next().unwrap(); s = &s[c.len_utf8()..];
        let v = match c { 'a'|'i'|'ä'|'y' => 1, 'e' => 2, 'o'|'ö' => 3, 'u'|'ü'|'w' => 4, 'm'|'b'|'p' => 5, 'f'|'v' => 6, 'l'|'t'|'d'|'n'|'s'|'z'|'ß' => 7, _ => 0 };
        if v != 0 { out.push(v); }
    }
    out.dedup(); out
}
/// Two bone IK in screen coordinates. Unreachable goals project onto the reach annulus.
pub fn ik(x: f64, y: f64, a: f64, b: f64) -> (f64, f64) {
    let d = x.hypot(y).clamp((a-b).abs()+1e-6, a+b-1e-6);
    let elbow = ((d*d-a*a-b*b)/(2.0*a*b)).clamp(-1.0,1.0).acos();
    let shoulder = y.atan2(x) - (b*elbow.sin()).atan2(a+b*elbow.cos());
    (shoulder.to_degrees()-90.0, elbow.to_degrees())
}
/// Bone lengths in rig units (1000 units = presenter height).
const UPPER: f64 = 185.0;
const FORE: f64 = 170.0;
/// Torso pivot and shoulder offsets, shared by the rig and the IK solver.
const TORSO: (f64, f64) = (250.0, 430.0);
const SHOULDER: (f64, f64) = (148.0, 8.0);
const HEAD_Y: f64 = -262.0;
#[derive(Clone, Copy, PartialEq)]
enum Role { Body, Mark, Detail }
struct Pal { face: Rgba, accent: Rgba, ink: Rgba, shade: Rgba, white: Rgba, tongue: Rgba, trim: Rgba }
fn mix(a: &Rgba, b: &Rgba, t: f64) -> Rgba {
    Rgba { r: a.r+(b.r-a.r)*t, g: a.g+(b.g-a.g)*t, b: a.b+(b.b-a.b)*t, a: 1.0 }
}
fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    format!("M {} {cy} A {rx} {ry} 0 1 0 {} {cy} A {rx} {ry} 0 1 0 {} {cy} Z", cx-rx,cx+rx,cx-rx)
}
fn capsule(w: f64, h: f64) -> String {
    format!("M {} 0 Q {} {} 0 {} Q {} {} {} 0 L {} {h} Q 0 {} {} {h} Z", -w/2.0,-w/2.0,-w/2.0,-w/2.0,w/2.0,-w/2.0,w/2.0,w/2.0,h+w/2.0,-w/2.0)
}
/// Rounded bar of thickness `t` between two points (brows, closed lips).
fn bar(x0: f64, y0: f64, x1: f64, y1: f64, t: f64) -> String {
    let (dx,dy)=(x1-x0,y1-y0); let l=dx.hypot(dy).max(1e-6);
    let (nx,ny)=(-dy/l*t/2.0,dx/l*t/2.0); let r=t/2.0;
    format!("M {:.1} {:.1} L {:.1} {:.1} A {r} {r} 0 0 0 {:.1} {:.1} L {:.1} {:.1} A {r} {r} 0 0 0 {:.1} {:.1} Z",
        x0+nx,y0+ny,x1+nx,y1+ny,x1-nx,y1-ny,x0-nx,y0-ny,x0+nx,y0+ny)
}
impl<'a> Ctx<'a> {
    fn presenter_check(&mut self, n: &Node, value: &str, choices: &[&str]) -> bool {
        if choices.contains(&value) { return true; }
        let hint = suggest(value, choices).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("choose {}", choices.join(", ")));
        self.err("E_PRESENTER", format!("unknown presenter value `{value}`"), hint, n.span); false
    }
    fn bone(&mut self, parent: usize, name: &str, x: f64, y: f64, s: f64) -> usize {
        let mut l = Layer::new(format!("{}~{name}", self.lid(parent)), Kind::Group);
        l.layout.pos=Pos::Abs; l.layout.x=Some(Len::px(x*s)); l.layout.y=Some(Len::px(y*s));
        l.layout.w=Some(Len::px(1000.0*s)); l.layout.h=Some(Len::px(1000.0*s)); l.style.origin=(0.0,0.0);
        l.tags.push("presenter-bone".into());
        self.add(Some(parent),l)
    }
    /// One piece of character art. `Body` pieces are outlined (line), jittered and shadowed (paper).
    fn art(&mut self, parent: usize, name: &str, d: String, c: &Rgba, role: Role, skin: &str, s: f64, seed: &mut u64) -> Option<usize> {
        if skin == "line" && role == Role::Detail { return None; }
        let mut c = c.clone();
        let d = if skin == "paper" {
            // Cut-paper edges: coarse flattening gives straight scissor facets, jitter adds hand irregularity.
            use kurbo::{BezPath, PathEl};
            let amp = if role == Role::Body { 2.6 } else { 1.0 };
            let tol = if role == Role::Body { 3.5 } else { 1.5 };
            let mut pts=vec![];
            if let Ok(path)=BezPath::from_svg(&d) {
                kurbo::flatten(path.iter(), tol, |el| match el {
                    PathEl::MoveTo(p) => pts.push(format!("M {:.1} {:.1}",p.x+(random(seed)-0.5)*amp,p.y+(random(seed)-0.5)*amp)),
                    PathEl::LineTo(p) => pts.push(format!("L {:.1} {:.1}",p.x+(random(seed)-0.5)*amp,p.y+(random(seed)-0.5)*amp)),
                    PathEl::ClosePath => pts.push("Z".into()), _=>{}
                });
            }
            // Every sheet of paper has its own slightly different tone.
            let k=1.0+(random(seed)-0.5)*0.10;
            c=Rgba {r:(c.r*k).min(1.0),g:(c.g*k).min(1.0),b:(c.b*k).min(1.0),a:c.a};
            pts.join(" ")
        } else { d };
        let mut l=Layer::new(format!("{}~{name}",self.lid(parent)),Kind::Path {d,closed:true,vb:(1000.0,1000.0)});
        l.layout.pos=Pos::Abs; l.layout.x=Some(Len::px(0.0)); l.layout.y=Some(Len::px(0.0));
        l.layout.w=Some(Len::px(1000.0*s)); l.layout.h=Some(Len::px(1000.0*s));
        if skin=="line" && role==Role::Body {
            // Outline weight is identical everywhere; the fill is the backdrop so limbs occlude the torso.
            l.style.stroke=Some(c.clone()); l.style.stroke_w=7.0*s;
            l.style.fill=Some(Paint::Solid(ColorRef::Lit(self.th.colors["bg"].clone())));
        } else { l.style.fill=Some(Paint::Solid(ColorRef::Lit(c))); }
        if skin=="paper" {
            let (ox,oy,a)=if role==Role::Body {(5.0,7.0,0.34)} else {(2.0,3.0,0.22)};
            let mut shadow=l.clone(); shadow.id.push_str("-shadow"); shadow.layout.x=Some(Len::px(ox*s)); shadow.layout.y=Some(Len::px(oy*s));
            shadow.style.fill=Some(Paint::Solid(ColorRef::Lit(Rgba {r:0.0,g:0.0,b:0.0,a})));
            self.add(Some(parent),shadow);
        }
        Some(self.add(Some(parent),l))
    }
    fn palette(&mut self, n: &Node, skin: &str) -> Pal {
        let th=self.th.colors.clone(); let th=&th;
        let ink=th["ink"].clone(); let white=th["paper"].clone();
        let warm=Rgba {r:0.93,g:0.68,b:0.50,a:1.0};
        let (face,accent)=match skin {
            "paper" => (self.color(n,"color").unwrap_or_else(||mix(&white,&warm,0.50)), self.color(n,"accent").unwrap_or_else(||th["warn"].clone())),
            _ => (self.color(n,"color").unwrap_or_else(||th["mint"].clone()), self.color(n,"accent").unwrap_or_else(||th["accent"].clone())),
        };
        if skin=="line" { return Pal {face:face.clone(),accent:face.clone(),ink:face.clone(),shade:face.clone(),white:face.clone(),tongue:face.clone(),trim:face} }
        let tongue=mix(&face,&th["danger"],0.55);
        Pal {shade:mix(&face,&ink,0.16),trim:mix(&accent,&ink,0.28),white,tongue,face,accent,ink}
    }
    pub fn presenter(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        let skin=prop(n,"skin","flat"); self.presenter_check(n,&skin,SKINS);
        let skin=if SKINS.contains(&skin.as_str()) {skin} else {"flat".into()};
        let mood=prop(n,"mood","neutral"); self.presenter_check(n,&mood,MOODS);
        let h=self.len(n,"h").map(|v| if v.unit=="%" {v.v*self.h/100.0} else {v.v}).unwrap_or(self.h*0.7).max(1.0);
        if h<self.h*0.25 {self.warn("W_PRESENTER_SMALL","presenter is smaller than 25% of the frame height","use h=25% or larger for readable expressions",n.span);}
        let s=h/1000.0;
        let mut seed=self.num(n,"seed").unwrap_or(1.0) as u64;
        let p=self.palette(n,&skin);
        let mut l=Layer::new(lid,Kind::Group); l.layout.pos=Pos::Abs; l.layout.w=Some(Len::px(500.0*s)); l.layout.h=Some(Len::px(h)); l.layout.anchor=(0.5,0.5);
        let root=self.add(Some(parent),l);
        let sk=skin.as_str(); let sd=&mut seed;
        // Legs: straight trousers, wide rounded shoes pointing outwards.
        for (x,name,out) in [(206.0,"leg_L",-1.0),(294.0,"leg_R",1.0)] {
            let leg=self.bone(root,name,x,715.0,s);
            self.art(leg,"trouser",capsule(98.0,205.0),&p.ink,Role::Body,sk,s,sd);
            self.art(leg,"shoe",ellipse(out*16.0,236.0,64.0,27.0),&p.shade,Role::Body,sk,s,sd);
        }
        let torso=self.bone(root,"torso",TORSO.0,TORSO.1,s);
        self.art(torso,"neck","M -44 -150 L 44 -150 L 44 -50 C 22 -20 -22 -20 -44 -50 Z".into(),&p.face,Role::Body,sk,s,sd);
        self.art(torso,"neck-shade","M -44 -122 L 44 -122 L 44 -88 C 20 -62 -20 -62 -44 -88 Z".into(),&p.shade,Role::Detail,sk,s,sd);
        self.art(torso,"shirt","M -166 4 C -166 -48 -124 -66 -74 -68 L -48 -68 C -30 -26 30 -26 48 -68 L 74 -68 C 124 -66 166 -48 166 4 L 150 308 C 60 334 -60 334 -150 308 Z".into(),&p.accent,Role::Body,sk,s,sd);
        self.art(torso,"hem","M -150 280 C -60 306 60 306 150 280 L 150 308 C 60 334 -60 334 -150 308 Z".into(),&p.trim,Role::Detail,sk,s,sd);
        let head=self.bone(torso,"head",0.0,HEAD_Y,s);
        self.art(head,"ears",format!("{} {}",ellipse(-146.0,14.0,25.0,34.0),ellipse(146.0,14.0,25.0,34.0)),&p.face,Role::Body,sk,s,sd);
        self.art(head,"face","M -148 -10 C -148 -112 -92 -158 0 -158 C 92 -158 148 -112 148 -10 C 148 78 98 156 0 158 C -98 156 -148 78 -148 -10 Z".into(),&p.face,Role::Body,sk,s,sd);
        self.art(head,"nose",ellipse(0.0,58.0,15.0,11.0),&p.shade,if skin=="line" {Role::Body} else {Role::Detail},sk,s,sd);
        self.art(head,"hair","M -158 8 C -198 -122 -112 -200 4 -188 C 132 -196 200 -98 158 8 C 152 -38 136 -74 108 -102 C 62 -92 -8 -84 -64 -108 C -94 -82 -132 -46 -158 8 Z".into(),&p.ink,Role::Body,sk,s,sd);
        let eyes=self.bone(head,"eyes",0.0,12.0,s);
        self.art(eyes,"sclera",format!("{} {}",ellipse(-56.0,0.0,31.0,35.0),ellipse(56.0,0.0,31.0,35.0)),&p.white,Role::Body,sk,s,sd);
        let pupils=self.bone(eyes,"pupils",0.0,0.0,s);
        self.art(pupils,"iris",format!("{} {}",ellipse(-56.0,2.0,16.0,20.0),ellipse(56.0,2.0,16.0,20.0)),&p.ink,Role::Mark,sk,s,sd);
        let mut brows=[0;4]; let mut rest=[0;4];
        let mouth_rest=self.bone(head,"mouth-rest",0.0,0.0,s);
        for m in 0..4 {
            brows[m]=self.bone(head,&format!("brows-{m}"),0.0,0.0,s);
            let (yo,yi)=match m {1=>(-52.0,-58.0),2=>(-56.0,-34.0),3=>(-70.0,-74.0),_=>(-44.0,-48.0)};
            self.art(brows[m],"ink",format!("{} {}",bar(-90.0,yo,-26.0,yi,17.0),bar(26.0,yi,90.0,yo,17.0)),&p.ink,Role::Mark,sk,s,sd);
            rest[m]=self.bone(mouth_rest,&format!("rest-{m}"),0.0,0.0,s);
            match m {
                1 => {
                    self.art(rest[m],"ink","M -50 88 L 50 88 Q 46 136 0 136 Q -46 136 -50 88 Z".into(),&p.ink,Role::Mark,sk,s,sd);
                    self.art(rest[m],"teeth","M -42 90 L 42 90 L 40 102 L -40 102 Z".into(),&p.white,Role::Detail,sk,s,sd);
                    self.art(rest[m],"tongue",ellipse(0.0,126.0,24.0,9.0),&p.tongue,Role::Detail,sk,s,sd);
                }
                2 => {self.art(rest[m],"ink",bar(-30.0,106.0,30.0,106.0,10.0),&p.ink,Role::Mark,sk,s,sd);}
                3 => {self.art(rest[m],"ink",ellipse(0.0,110.0,21.0,29.0),&p.ink,Role::Mark,sk,s,sd);}
                _ => {self.art(rest[m],"ink","M -38 96 Q 0 112 38 96 L 40 106 Q 0 132 -40 106 Z".into(),&p.ink,Role::Mark,sk,s,sd);}
            }
            self.set_base(brows[m],Ch::Opacity,TV::N(if MOODS[m]==mood {1.0}else{0.0}));
            self.set_base(rest[m],Ch::Opacity,TV::N(if MOODS[m]==mood {1.0}else{0.0}));
        }
        let mut mouth=[0;8];
        mouth[0]=mouth_rest;
        for v in 1..8 {
            mouth[v]=self.bone(head,&format!("mouth-{v}"),0.0,0.0,s);
            match v {
                1 => {self.art(mouth[v],"lip",ellipse(0.0,108.0,35.0,32.0),&p.ink,Role::Mark,sk,s,sd);
                      self.art(mouth[v],"tongue",ellipse(0.0,126.0,21.0,8.0),&p.tongue,Role::Detail,sk,s,sd);}
                2 => {self.art(mouth[v],"lip",ellipse(0.0,106.0,48.0,17.0),&p.ink,Role::Mark,sk,s,sd);
                      self.art(mouth[v],"teeth","M -38 98 L 38 98 L 36 105 L -36 105 Z".into(),&p.white,Role::Detail,sk,s,sd);}
                3 => {self.art(mouth[v],"lip",ellipse(0.0,110.0,25.0,31.0),&p.ink,Role::Mark,sk,s,sd);
                      self.art(mouth[v],"tongue",ellipse(0.0,126.0,13.0,6.0),&p.tongue,Role::Detail,sk,s,sd);}
                4 => {self.art(mouth[v],"lip",ellipse(0.0,108.0,15.0,17.0),&p.ink,Role::Mark,sk,s,sd);}
                5 => {self.art(mouth[v],"lip",bar(-38.0,106.0,38.0,106.0,11.0),&p.ink,Role::Mark,sk,s,sd);}
                6 => {self.art(mouth[v],"lip",ellipse(0.0,108.0,38.0,16.0),&p.ink,Role::Mark,sk,s,sd);
                      self.art(mouth[v],"teeth","M -30 98 L 30 98 L 28 109 L -28 109 Z".into(),&p.white,Role::Detail,sk,s,sd);}
                _ => {self.art(mouth[v],"lip",ellipse(0.0,108.0,35.0,21.0),&p.ink,Role::Mark,sk,s,sd);
                      self.art(mouth[v],"teeth","M -28 97 L 28 97 L 26 106 L -26 106 Z".into(),&p.white,Role::Detail,sk,s,sd);
                      self.art(mouth[v],"tongue",ellipse(0.0,118.0,17.0,7.0),&p.tongue,Role::Detail,sk,s,sd);}
            }
            self.set_base(mouth[v],Ch::Opacity,TV::N(0.0));
        }
        let mut arms=[[0;3];2]; let mut fingers=[[0;5];2];
        for side in 0..2 {
            let suffix=if side==0 {"L"} else {"R"};
            let sgn=if side==0 {-1.0} else {1.0};
            let upper=self.bone(torso,&format!("upper_arm_{suffix}"),sgn*SHOULDER.0,SHOULDER.1,s);
            self.art(upper,"sleeve",capsule(80.0,UPPER),&p.accent,Role::Body,sk,s,sd);
            let fore=self.bone(upper,&format!("forearm_{suffix}"),0.0,UPPER,s);
            self.art(fore,"skin",capsule(56.0,FORE),&p.face,Role::Body,sk,s,sd);
            self.art(fore,"cuff","M -38 -8 L 38 -8 L 36 30 L -36 30 Z".into(),&p.trim,Role::Body,sk,s,sd);
            let hand=self.bone(fore,&format!("hand_{suffix}"),0.0,FORE,s);
            self.art(hand,"palm",ellipse(0.0,18.0,35.0,41.0),&p.face,Role::Body,sk,s,sd);
            self.art(hand,"thumb",ellipse(-sgn*32.0,10.0,13.0,23.0),&p.face,Role::Body,sk,s,sd);
            for f in 0..5 {
                fingers[side][f]=self.bone(hand,&format!("finger-{f}"),(f as f64-2.0)*14.0,34.0,s);
                self.art(fingers[side][f],"digit",capsule(13.0,44.0-(f as f64-2.0).abs()*6.0),&p.face,Role::Body,sk,s,sd);
                self.set_base(fingers[side][f],Ch::Opacity,TV::N(0.0));
            }
            arms[side]=[upper,fore,hand];
            self.set_base(upper,Ch::Rot,TV::N(sgn*-8.0));
            self.set_base(fore,Ch::Rot,TV::N(sgn*12.0));
        }
        let rig=Rig {root,scene:si,source:n.clone(),scale:s,torso,head,eyes,pupils,brows,rest,mouth,arms,fingers};
        self.presenter_idle(&rig,seed); self.presenters.push(rig); root
    }
    fn presenter_idle(&mut self, r: &Rig, mut seed: u64) {
        let start=self.scenes[r.scene].start;
        let end=start+self.scenes[r.scene].dur;
        for i in 0..=((end-start)*2.0).ceil() as usize {
            let t=(start+i as f64*0.5).min(end);
            let phase=(t-start)*std::f64::consts::TAU/4.0;
            self.track(r.torso,Ch::Sy).keys.push(Key {t,v:TV::N(1.006-0.006*phase.cos()),ease:EASE});
            self.track(r.torso,Ch::Rot).keys.push(Key {t,v:TV::N(0.9*(phase*0.5).sin()),ease:EASE});
            self.track(r.head,Ch::Rot).keys.push(Key {t,v:TV::N(SWAY*phase.sin()),ease:EASE});
        }
        // Entrance: the presenter springs in from the side, so there is motion on frame one.
        self.set_base(r.root,Ch::Scale,TV::N(0.9));
        self.set_base(r.root,Ch::X,TV::N(70.0*r.scale));
        let spring=Ease::Spring {stiffness:170.0,damping:15.0,mass:1.0,velocity:0.0};
        self.track(r.root,Ch::Scale).to(start,start+0.7,TV::N(1.0),spring.clone());
        self.track(r.root,Ch::X).to(start,start+0.7,TV::N(0.0),spring);
        let mut t=start+2.5+random(&mut seed)*2.5;
        while t+0.12<end {
            self.track(r.eyes,Ch::Sy).to(t,t+0.055,TV::N(0.08),EASE);
            self.track(r.eyes,Ch::Sy).to(t+0.065,t+0.12,TV::N(1.0),EASE);
            t+=2.5+random(&mut seed)*2.5;
        }
        let mut schedule=vec![(0u64,0usize)];
        for say in &self.tl.scenes[r.scene].says {
            for word in &say.words {
                let first=(word.start*self.fps).ceil() as u64;
                let last=(word.end*self.fps).floor() as u64;
                // Talk bob: the head dips on every word, which reads as emphasis.
                let ws=word.start.max(start);
                if ws+0.22<end {
                    self.track(r.head,Ch::Y).to(ws,ws+0.07,TV::N(-5.0*r.scale),EASE);
                    self.track(r.head,Ch::Y).to(ws+0.07,ws+0.22,TV::N(0.0),EASE);
                }
                let vs=visemes(&word.text);
                if vs.is_empty() || last<first+2 {continue;}
                let count=vs.len().min(((last-first)/2) as usize);
                for i in 0..count {
                    let f=first+(last-first)*i as u64/count as u64;
                    if f>=schedule.last().unwrap().0+2 {schedule.push((f,vs[i*vs.len()/count]));}
                }
                if last>=schedule.last().unwrap().0+2 {schedule.push((last,0));}
            }
        }
        // Every state survives at least two frames, including rests and word boundaries.
        let mut prev=0;
        for (frame,v) in schedule.into_iter().skip(1) {
            if v==prev {continue;}
            let t=frame as f64/self.fps;
            for (state,value) in [(prev,0.0),(v,1.0)] {
                let tr=self.track(r.mouth[state],Ch::Opacity);
                if tr.keys.is_empty() {tr.keys.push(Key {t:0.0,v:tr.base.clone(),ease:Ease::Hold});}
                tr.keys.push(Key {t,v:TV::N(value),ease:Ease::Hold});
            }
            prev=v;
        }
    }
    fn presenter_target(&mut self, r: &Rig, name: &str, n: &Node) -> Option<[f64;4]> {
        let name=name.trim_start_matches('#');
        let key=format!("{name}@{}",r.scene);
        if !self.names.contains_key(&key) {
            let mut ids: Vec<&str>=self.names.iter().filter(|(k,v)|v.scene==r.scene&&!k.contains('@')).map(|(k,_)|k.as_str()).collect(); ids.sort();
            let hint=suggest(name,&ids).map(|s|format!("did you mean `#{s}`?")).unwrap_or_else(||"use an element id in this scene".into());
            self.err("E_PRESENTER",format!("presenter target `#{name}` not found"),hint,n.span); return None;
        }
        self.need_geo=true;
        self.geo.as_ref().and_then(|g|g.rects.get(&key)).copied()
    }
    fn presenter_box(&self,r:&Rig)->[f64;4] {
        let key=format!("{}@{}",id_arg(&r.source).unwrap_or_default(),r.scene);
        self.geo.as_ref().and_then(|g|g.rects.get(&key)).copied().unwrap_or([0.0,0.0,500.0*r.scale,1000.0*r.scale])
    }
    fn gaze(&mut self,r:&Rig,target:[f64;4],t:f64) {
        let b=self.presenter_box(r);
        let dx=(target[0]+target[2]/2.0-b[0]-250.0*r.scale)/(400.0*r.scale);
        let dy=(target[1]+target[3]/2.0-b[1]-(TORSO.1+HEAD_Y)*r.scale)/(400.0*r.scale);
        self.track(r.pupils,Ch::X).to(t,t+0.24,TV::N(dx.clamp(-1.0,1.0)*13.0*r.scale),EASE);
        self.track(r.pupils,Ch::Y).to(t,t+0.24,TV::N(dy.clamp(-1.0,1.0)*8.0*r.scale),EASE);
        let turn=dx.clamp(-1.0,1.0)*9.0;
        self.track(r.head,Ch::Rot).to(t,t+0.35,TV::N(turn),EASE);
        let start=self.scenes[r.scene].start;
        let end=start+self.scenes[r.scene].dur;
        let mut at=t+0.85;
        while at<=end {
            let sway=SWAY*((at-start)*std::f64::consts::TAU/4.0).sin();
            self.track(r.head,Ch::Rot).keys.push(Key {t:at,v:TV::N(turn+sway),ease:EASE});
            at+=0.5;
        }
    }
    pub fn presenter_ready(&mut self) {
        for r in self.presenters.clone() {
            if let Some(look)=r.source.props.get("look") {
                if let Some(b)=self.presenter_target(&r,&look.raw(),&r.source) {self.gaze(&r,b,self.scenes[r.scene].start);}
            }
        }
    }
    /// Approach `value`, hold, then return to the pose held at `t`.
    fn pose(&mut self, i:usize,ch:Ch,t:f64,value:f64,end:f64) { self.pose_hold(i,ch,t,value,end,0.85); }
    fn pose_hold(&mut self, i:usize,ch:Ch,t:f64,value:f64,end:f64,hold:f64) {
        if t>=end {return;}
        let base=self.track(i,ch).eval(t);
        let tr=self.track(i,ch);
        tr.to(t,(t+0.32).min(end),TV::N(value),EASE);
        tr.to((t+hold).min(end),(t+hold+0.35).min(end),base,EASE);
    }
    pub fn presenter_event(&mut self,e:&Ev) {
        let n=&e.node;
        let name=id_arg(n).or_else(||e.target.clone()).unwrap_or_default();
        let r=self.presenters.iter().find(|r|r.scene==e.scene && self.layers[r.root].name.as_deref()==Some(&name)).cloned();
        let Some(r)=r else {
            let ids:Vec<&str>=self.presenters.iter().filter(|r|r.scene==e.scene).filter_map(|r|self.layers[r.root].name.as_deref()).collect();
            let hint=suggest(&name,&ids).map(|s|format!("did you mean `#{s}`?")).unwrap_or_else(||"use `gesture #presenter wave` or `mood #presenter happy`".into());
            self.err("E_PRESENTER",format!("presenter `#{name}` not found"),hint,n.span);return;
        };
        let args:Vec<String>=n.args.iter().map(|v|v.raw()).collect();
        let words:Vec<&str>=args.iter().filter(|s|!s.starts_with('#')).map(String::as_str).collect();
        let verb=words.first().copied().unwrap_or("");
        if n.kind=="mood" {
            if !self.presenter_check(n,verb,MOODS) {return;}
            for m in 0..4 {for i in [r.brows[m],r.rest[m]] {self.track(i,Ch::Opacity).jump(e.t,TV::N(if MOODS[m]==verb {1.0}else{0.0}));}}
            return;
        }
        if !self.presenter_check(n,verb,GESTURES) {return;}
        let t=e.t; let end=self.scenes[e.scene].start+self.scenes[e.scene].dur;
        let target=args.iter().filter(|s|s.starts_with('#')).nth(1);
        let b=if let Some(target)=target {self.presenter_target(&r,target,n)} else {None};
        if matches!(verb,"point"|"present") && target.is_none() {self.err("E_PRESENTER",format!("`{verb}` needs a target"),format!("gesture #{name} {verb} #target"),n.span);return;}
        if let Some(b)=b {self.gaze(&r,b,t);}
        match verb {
            "nod" => {self.pose(r.head,Ch::Rot,t,11.0,end);self.track(r.head,Ch::Rot).to((t+0.4).min(end),(t+0.65).min(end),TV::N(-5.0),EASE);self.track(r.head,Ch::Rot).to((t+0.65).min(end),(t+1.1).min(end),TV::N(0.0),EASE);}
            "shrug" => {
                for side in 0..2 {self.pose(r.arms[side][0],Ch::Y,t,-22.0*r.scale,end);self.pose(r.arms[side][0],Ch::Rot,t,if side==0 {42.0}else{-42.0},end);self.pose(r.arms[side][1],Ch::Rot,t+0.06,if side==0 {80.0}else{-80.0},end);}
            }
            _ => {
                let body=self.presenter_box(&r);
                let side=if b.map(|b|b[0]+b[2]/2.0<body[0]+250.0*r.scale).unwrap_or(false) {0}else{1};
                let sgn=if side==0 {-1.0} else {1.0};
                let hold=match verb {"point"|"present"=>1.6,"think"=>1.3,"wave"=>1.5,_=>1.2};
                let (a,f)=if verb=="think" {ik(-sgn*124.0,-52.0,UPPER,FORE)} else if let Some(b)=b {
                    let sx=body[0]+(TORSO.0+sgn*SHOULDER.0)*r.scale;
                    let sy=body[1]+(TORSO.1+SHOULDER.1)*r.scale;
                    ik((b[0]+b[2]/2.0-sx)/r.scale,(b[1]+b[3]/2.0-sy)/r.scale,UPPER,FORE)
                } else {(sgn*-65.0,sgn*-105.0)};
                self.pose_hold(r.arms[side][0],Ch::Rot,t,a,end,hold);
                self.pose_hold(r.arms[side][1],Ch::Rot,t+0.06,f,end,hold);
                self.pose_hold(r.arms[side][2],Ch::Rot,t+0.12,match verb {"present"=>-sgn*50.0,"think"=>sgn*30.0,_=>0.0},end,hold);
                let count=if verb=="count" {
                    match words.get(1).and_then(|v|v.parse::<usize>().ok()).filter(|v|(1..=5).contains(v)) {
                        Some(v)=>v,None=>{self.err("E_PRESENTER","count needs an integer from 1 to 5","write `gesture #id count 3`",n.span);return;}
                    }
                } else if verb=="point" {1} else if matches!(verb,"wave"|"present") {5} else {0};
                for f in 0..5 {
                    // A pointing hand shows only its (longest) middle digit.
                    let show=if verb=="point" {f==2} else {f<count};
                    let tr=self.track(r.fingers[side][f],Ch::Opacity);
                    tr.jump(t+0.15,TV::N(if show {1.0}else{0.0}));
                    tr.jump((t+hold+0.2).min(end),TV::N(0.0));
                }
                if verb=="wave" {
                    let tr=self.track(r.arms[side][2],Ch::Rot);
                    for (i,dt) in [0.25,0.4,0.55,0.7,0.85,1.0,1.15].iter().enumerate() {
                        tr.to((t+dt-0.13).min(end),(t+dt).min(end),TV::N(if i%2==0 {-28.0} else {28.0}),EASE);
                    }
                    tr.to((t+1.3).min(end),(t+1.45).min(end),TV::N(0.0),EASE);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn german_english_visemes() {
        assert_eq!(visemes("A e O ü m f n"),vec![1,2,3,4,5,6,7]);
        assert_eq!(visemes("Schiff"),vec![4,1,6]);
        assert_eq!(visemes("three"),vec![7,2]);
        assert_eq!(visemes("Bäu"),vec![5,3]);
        assert!(visemes("?!").is_empty());
    }
    #[test] fn ik_reaches() {
        for (x,y) in [(100.0,180.0),(-200.0,-80.0),(0.0,350.0),(10.0,20.0)] {
            let (a,b)=ik(x,y,UPPER,FORE); let a=(a+90.0).to_radians();let b=b.to_radians();
            let px=UPPER*a.cos()+FORE*(a+b).cos();let py=UPPER*a.sin()+FORE*(a+b).sin();
            assert!((px-x).hypot(py-y)<x.hypot(y)*0.02);
        }
    }
}
