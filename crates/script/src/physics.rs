//! Compile-time deterministic convex impulse solver. Fixed 240 Hz, output-frame samples.
use crate::values::Ease;
use crate::{fracture::Point, ir::*, lower::*};
#[derive(Clone, Debug)]
struct Body { p: Point, v: Point, poly: Vec<Point>, angle: f64, spin: f64, delay: f64, quiet: usize, asleep: bool }
fn vertices(b: &Body) -> Vec<Point> { let (s,c)=b.angle.sin_cos(); b.poly.iter().map(|p| [b.p[0]+c*p[0]-s*p[1],b.p[1]+s*p[0]+c*p[1]]).collect() }
fn contact(a: &[Point], b: &[Point]) -> Option<(Point,f64)> {
    let mut best=([0.,0.],f64::INFINITY);
    for poly in [a,b] { for i in 0..poly.len() { let q=poly[(i+1)%poly.len()]; let p=poly[i]; let len=(q[0]-p[0]).hypot(q[1]-p[1]); if len<1e-9 {continue} let n=[-(q[1]-p[1])/len,(q[0]-p[0])/len];
        let project=|ps:&[Point]| ps.iter().map(|p|p[0]*n[0]+p[1]*n[1]).fold((f64::INFINITY,f64::NEG_INFINITY),|(lo,hi),v|(lo.min(v),hi.max(v)));
        let (al,ah)=project(a); let (bl,bh)=project(b); let depth=ah.min(bh)-al.max(bl); if depth<=0. {return None} if depth<best.1 {best=(n,depth);}
    }} Some(best)
}
fn step(bs: &mut [Body], t:f64, dt:f64, gravity:f64, bounce:f64, friction:f64, walls:Option<[f64;4]>) {
    let old:Vec<_>=bs.iter().map(|b|b.p).collect();
    for b in bs.iter_mut() { if t<b.delay || b.asleep {continue} b.v[1]+=gravity*dt; for k in 0..2 {b.p[k]+=b.v[k]*dt;} b.angle+=b.spin*dt; b.spin*=(-friction*dt).exp(); }
    for _ in 0..8 {
        for i in 0..bs.len() { if t<bs[i].delay {continue} for j in i+1..bs.len() { if t<bs[j].delay {continue}
            let (left,right)=bs.split_at_mut(j); let a=&mut left[i]; let b=&mut right[0]; if a.asleep && b.asleep { continue; }
            if let Some((mut n,d))=contact(&vertices(a),&vertices(b)) { if (b.p[0]-a.p[0])*n[0]+(b.p[1]-a.p[1])*n[1]<0. {n=[-n[0],-n[1]];}
                for k in 0..2 {a.p[k]-=n[k]*(d+1e-6)*0.5;b.p[k]+=n[k]*(d+1e-6)*0.5;}
                let speed=(b.v[0]-a.v[0])*n[0]+(b.v[1]-a.v[1])*n[1];
                if speed < -30. { a.asleep=false; b.asleep=false; a.quiet=0; b.quiet=0; }
                if speed<0. {let impulse=-(1.+if speed.abs()<30. {0.} else {bounce})*speed/2.; for k in 0..2 {a.v[k]-=impulse*n[k]; b.v[k]+=impulse*n[k];} a.v[0]*=1.-friction*0.03;b.v[0]*=1.-friction*0.03; a.spin*=0.92; b.spin*=0.92; }
            }
        }}
        if let Some(w)=walls {for b in bs.iter_mut() {if t<b.delay {continue} let vs=vertices(b); for axis in 0..2 { let lo=vs.iter().map(|p|p[axis]).fold(f64::INFINITY,f64::min); let hi=vs.iter().map(|p|p[axis]).fold(f64::NEG_INFINITY,f64::max); let min=w[axis];let max=min+w[axis+2];
            let hit=if lo<min {b.p[axis]+=min-lo;b.v[axis]<0.} else if hi>max {b.p[axis]-=hi-max;b.v[axis]>0.} else {false};
            if hit {b.v[axis]*=if b.v[axis].abs()<30. {0.} else {-bounce}; b.v[1-axis]*=1.-friction*0.12;b.spin*=0.85;}
        }}}
    }
    for (b,p) in bs.iter_mut().zip(old) {if t<b.delay {continue} if (b.p[0]-p[0]).hypot(b.p[1]-p[1])<0.015 && b.spin.abs()<0.01 {b.quiet+=1;} else {b.quiet=0;b.asleep=false;} if b.quiet>=120 {b.asleep=true;b.v=[0.,0.];b.spin=0.;}}
}
fn number(n:&crate::ast::Node,k:&str,d:f64)->Result<f64,String> { match n.props.get(k) {None=>Ok(d),Some(v)=>{let raw=v.raw(); let s=raw.trim_end_matches("deg"); let x=s.parse::<f64>().map_err(|_|format!("{k} must be a finite number"))?; if x.is_finite(){Ok(x)}else{Err(format!("{k} must be finite"))}}} }
impl<'a> Ctx<'a> {
    pub fn physics_verb(&mut self,e:&Ev) {if let Err(msg)=self.physics_inner(e) {self.err("E_PHYSICS",msg,"use drop #id gravity=2400 bounce=0.35; targets and colliders must be laid out in this scene",e.node.span);}}
    fn physics_inner(&mut self,e:&Ev)->Result<(),String> {
        let n=&e.node; let kind=n.kind.as_str();
        let allowed=["gravity","bounce","friction","stagger","walls","at","from","angle","damping","dir","speed","pieces","seed","origin","force","dur","spin","scatter"];
        for k in n.props.keys() {if !allowed.contains(&k.as_str()) {return Err(format!("unknown physics parameter `{k}`; did you mean {}?",crate::values::suggest(k,&allowed).unwrap_or("gravity")));}}
        let name=id_arg(n).or_else(||e.target.clone()).ok_or("physics needs a target #id")?;
        let i=self.names.get(&format!("{name}@{}",e.scene)).map(|v|v.idx).ok_or_else(||{let candidates:Vec<_>=self.names.iter().filter(|(_,v)|v.scene==e.scene).map(|(k,_)|k.split('@').next().unwrap()).collect(); format!("unknown physics target #{name}; {}",crate::values::suggest(&name,&candidates).map(|s|format!("did you mean #{s}?")).unwrap_or("check the scene's element ids".into()))})?;
        let gravity=number(n,"gravity",if kind=="push" {0.}else{2400.})?;let bounce=number(n,"bounce",0.35)?;let friction=number(n,"friction",0.6)?;let damping=number(n,"damping",0.15)?;
        if !(0. ..=1e6).contains(&gravity) || !(0. ..=1.).contains(&bounce) || !(0. ..=1.).contains(&friction) || damping<0. {return Err("gravity/damping must be nonnegative; bounce/friction must be in 0..1".into())}
        let duration=if let Some(v)=n.props.get("dur") {crate::values::parse_dur(&v.raw()).map(|d|d.secs(self.fps)).map_err(|_|"invalid physics dur")?}else{self.scenes[e.scene].start+self.scenes[e.scene].dur-e.t};
        let stagger=if let Some(v)=n.props.get("stagger") {crate::values::parse_dur(&v.raw()).map(|d|d.secs(self.fps)).map_err(|_|"invalid stagger")?}else{0.};
        if duration<=0. || duration>120. || stagger<0. {return Err("dur must be in (0,120s]; stagger must be nonnegative".into())}
        let count=number(n,"pieces",24.)?;let seed=number(n,"seed",7.)?;
        if count.fract()!=0. || !(2. ..=128.).contains(&count) || seed<0. || seed.fract()!=0. {return Err("pieces must be an integer 2..128; seed a nonnegative integer".into())}
        let angle=number(n,"angle",35.)?.to_radians();let dir=number(n,"dir",0.)?.to_radians();let speed=number(n,"speed",900.)?;let force=number(n,"force",1600.)?;
        if !(0. ..=1e6).contains(&speed) || !(0. ..=1e6).contains(&force) {return Err("speed/force must be nonnegative".into())}
        let spin=number(n,"spin",0.)?.to_radians();let scatter=number(n,"scatter",0.)?;
        if !spin.is_finite() || !scatter.is_finite() {return Err("spin/scatter must be finite".into())}
        let walls=n.props.get("walls").map(|v|v.raw()).unwrap_or("frame".into());if walls!="frame" && walls!="none" {return Err("walls must be frame or none".into())}
        let args:Vec<_>=n.args.iter().map(|v|v.raw()).collect();
        let mut rest=args.iter();
        if args.first().is_some_and(|s|s.starts_with('#')) {rest.next();}
        let extra:Vec<_>=rest.collect();
        if !extra.is_empty() && !(extra.len()==2 && extra[0]=="into" && extra[1].starts_with('#')) {return Err("expected #target, optionally followed by into #container".into())}
        let into=args.iter().position(|s|s=="into").and_then(|p|args.get(p+1)).cloned();
        let collider=into.clone().or_else(||n.props.get("at").map(|v|v.raw()));
        let hook=n.props.get("from").map(|v|v.raw());
        for id in collider.iter().chain(hook.iter()) {if !self.names.contains_key(&format!("{}@{}",id.trim_start_matches('#'),e.scene)){return Err(format!("unknown physics collider {id}"));}}
        if kind=="swing" && hook.is_none() {return Err("swing needs from=#hook".into())}
        let origin=if let Some(v)=n.props.get("origin") {let s=v.raw();let p:Vec<_>=s.split(',').map(|p|p.trim().trim_end_matches('%').parse::<f64>().map(|v|v/100.)).collect::<Result<_,_>>().map_err(|_|"origin needs x%,y%")?;if p.len()!=2 || p.iter().any(|v|!v.is_finite() || !(0. ..=1.).contains(v)){return Err("origin needs percentages in 0..100%".into())} [p[0],p[1]]}else{[0.5,0.5]};
        self.need_geo=true;let Some(geo)=self.geo.clone() else{return Ok(())};
        let rect=|idx:usize| geo.rects.get(&self.layers[idx].id).copied();
        let source=rect(i).ok_or("target has no native layout rect")?;
        let lookup=|id:&str|geo.rects.get(&format!("{}@{}",id.trim_start_matches('#'),e.scene)).copied().ok_or("collider has no layout rect");
        let mut bounds=if walls=="frame" {Some([0.,0.,self.w,self.h])}else{None};
        if let Some(id)=collider {let r=lookup(&id)?; bounds=Some(if into.is_some(){r}else{[r[0],-100000.,r[2],100000.+r[1]]});}
        let pivot=hook.as_ref().map(|id|lookup(id)).transpose()?.map(|r|[r[0]+r[2]/2.,r[1]+r[3]/2.]);
        let mut ids=if self.kids[i].is_empty(){vec![i]}else{self.kids[i].clone()};
        let mut rects:Vec<_>=ids.iter().map(|&id|rect(id).ok_or("body has no native layout rect")).collect::<Result<_,_>>()?;
        let mut polys:Vec<Vec<Point>>=ids.iter().zip(&rects).map(|(&id,r)| if matches!(self.layers[id].kind,Kind::Ellipse) {(0..32).map(|j|{let a=j as f64*std::f64::consts::TAU/32.; [r[2]/2.*(1.+a.cos()),r[3]/2.*(1.+a.sin())]}).collect()} else if matches!(self.layers[id].kind,Kind::Text{..}) {vec![[0.,r[3]*0.20],[r[2],r[3]*0.20],[r[2],r[3]*0.81],[0.,r[3]*0.81]]} else {vec![[0.,0.],[r[2],0.],[r[2],r[3]],[0.,r[3]]]}).collect();
        if kind=="shatter" {
            if !self.kids[i].is_empty() {return Err("shatter v1 needs a leaf (text, image, rect or path); target the leaf inside the group".into())}
            polys=crate::fracture::cells(source[2],source[3],count as usize,seed as u64);
            let original=self.layers[i].clone(); self.layers[i].kind=Kind::Group; self.layers[i].layout.w=Some(Len::px(source[2])); self.layers[i].layout.h=Some(Len::px(source[3])); self.layers[i].style=Style {origin:(0.5,0.5),..Default::default()};
            let mut intact=original.clone();intact.id=format!("{}-intact",original.id);intact.name=None;intact.layout=Layout{pos:Pos::Fill,..Default::default()};let intact_id=self.add(Some(i),intact);
            self.track(intact_id,Ch::Opacity).jump(e.t,TV::N(0.));
            ids.clear();rects.clear();
            for (j,p) in polys.iter().enumerate() {let mut l=original.clone();l.id=format!("{}-shard-{j}",original.id);l.name=None;l.layout=Layout{pos:Pos::Fill,..Default::default()};l.style.clip=Some(ClipShape::Polygon(p.clone()));let id=self.add(Some(i),l);self.track(id,Ch::Opacity).base=TV::N(0.);self.track(id,Ch::Opacity).jump(e.t,TV::N(1.)); ids.push(id);rects.push(source);}
        }
        if ids.is_empty() || ids.len()>256 {return Err("physics needs 1..256 bodies".into())}
        let mut bodies=vec![];let mut starts=vec![];
        for (j,(&id,r)) in ids.iter().zip(&rects).enumerate() {
            if r[2]<=0. || r[3]<=0. {return Err("physics body has an empty layout rect".into())}
            if let Some(w)=bounds {if r[2]>w[2] || r[3]>w[3] {return Err("body is larger than its container; enlarge the container or shrink the body".into())}}
            let p=&polys[j];let c=[p.iter().map(|p|p[0]).sum::<f64>()/p.len() as f64,p.iter().map(|p|p[1]).sum::<f64>()/p.len() as f64];
            self.layers[id].style.origin=(c[0]/r[2],c[1]/r[3]);
            let x=self.track(id,Ch::X).eval(e.t).n();let y=self.track(id,Ch::Y).eval(e.t).n();let rot=self.track(id,Ch::Rot).eval(e.t).n().to_radians();
            let pos=[r[0]+c[0]+x,r[1]+c[1]+y];starts.push((pos,x,y));
            let v=if kind=="shatter" {let d=[pos[0]-source[0]-origin[0]*source[2],pos[1]-source[1]-origin[1]*source[3]];let len=d[0].hypot(d[1]).max(1.);[d[0]/len*force,d[1]/len*force]} else if kind=="push" {[dir.cos()*speed,dir.sin()*speed]}else{[(j as f64*1.7+1.).sin()*scatter,0.]};
            bodies.push(Body{p:pos,v,poly:p.iter().map(|p|[p[0]-c[0],p[1]-c[1]]).collect(),angle:rot,spin:if kind=="shatter" {(j as f64*2.399).sin()*3.}else{(j as f64*2.399).sin()*spin},delay:j as f64*stagger,quiet:0,asleep:false});
        }
        let frames=(duration*self.fps).ceil() as usize;let mut elapsed=0.;
        for frame in 0..=frames {let time=(frame as f64/self.fps).min(duration);
            while elapsed+1e-9<time {let dt=(1./240_f64).min(time-elapsed);step(&mut bodies,elapsed,dt,gravity,bounce,friction,bounds);elapsed+=dt;}
            for (j,b) in bodies.iter().enumerate() {let (start,x,y)=starts[j];let mut vals=[b.p[0]-start[0]+x,b.p[1]-start[1]+y,b.angle.to_degrees()];
                if kind=="swing" {let p=pivot.unwrap();let len=(start[0]-p[0]).hypot(start[1]-p[1]).max(1.);let a=angle*(-damping*time).exp()*( (gravity/len).sqrt()*time).cos();let base=(start[0]-p[0]).atan2(start[1]-p[1]); vals=[p[0]+len*(base+a).sin()-start[0]+x,p[1]+len*(base+a).cos()-start[1]+y,-a.to_degrees()];}
                for (ch,v) in [Ch::X,Ch::Y,Ch::Rot].into_iter().zip(vals) {let tr=self.track(ids[j],ch);if frame==0 {tr.to(e.t,e.t+1e-3,TV::N(v),Ease::Linear);tr.keys.retain(|k|k.t<=e.t);tr.keys.push(Key{t:e.t,v:TV::N(v),ease:Ease::Linear});}else if !b.asleep || kind=="swing" || tr.keys.last().is_some_and(|k|k.v!=TV::N(v)) {tr.keys.push(Key{t:e.t+time,v:TV::N(v),ease:Ease::Linear});}}
            }
        } Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    fn body() -> Body {Body{p:[50.,20.],v:[10.,0.],poly:vec![[-5.,-5.],[5.,-5.],[5.,5.],[-5.,5.]],angle:0.,spin:0.,delay:0.,quiet:0,asleep:false}}
    #[test] fn settles_inside_and_is_deterministic() {let run=||{let mut b=vec![body()];for i in 0..2400 {step(&mut b,i as f64/240.,1./240.,2400.,0.35,0.6,Some([0.,0.,100.,100.]));}b};let a=run();let b=run();assert_eq!(a[0].p,b[0].p);assert!(a[0].asleep);for p in vertices(&a[0]) {assert!(p[0]>=0. && p[0]<=100. && p[1]>=0. && p[1]<=100.);}let mut rest=a.clone();step(&mut rest,11.,1./240.,2400.,0.35,0.6,Some([0.,0.,100.,100.]));assert_eq!(rest[0].p,a[0].p);}
}
