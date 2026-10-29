//! Deterministic digital/tape looks on premultiplied RGBA8. Sampling clamps at edges;
//! spatial looks retain source coverage, so filtering never invents opaque margins.
use crate::look::{hash, unit, FxCtx, NPARAM, TIME};
use agentfx_ir::Effect;

fn rgb(p: &[u8]) -> [f32; 3] {
    if p[3] == 0 { [0.0; 3] } else { std::array::from_fn(|i| p[i] as f32 / p[3] as f32) }
}
fn luma(c: [f32; 3]) -> f32 { c[0]*0.2126 + c[1]*0.7152 + c[2]*0.0722 }
fn put(p: &mut [u8], c: [f32; 3]) {
    for i in 0..3 { p[i] = (c[i].clamp(0.0,1.0)*p[3] as f32).round() as u8; }
}
fn sample(b: &[u8], w: usize, h: usize, x: f64, y: f64) -> [f32; 3] {
    let x=x.round().clamp(0.0,(w-1) as f64) as usize;
    let y=y.round().clamp(0.0,(h-1) as f64) as usize;
    rgb(&b[(y*w+x)*4..][..4])
}

pub fn apply(e: &Effect, p: &[f64; NPARAM], b: &mut [u8], w: usize, h: usize, cx: &FxCtx) {
    if w==0 || h==0 { return; }
    if matches!(e, Effect::Rgbsplit{..}) && p[2]<=0.0 || matches!(e, Effect::Glitch{..}|Effect::Vhs{..}) && p[0]<=0.0 { return; }
    if let Effect::Dither {mode,size,palette,..}=e { dither(b,w,h,p[0],mode,*size,palette.as_deref(),cx); return; }
    if let Effect::Ascii {charset,color,bg,..}=e { ascii(b,w,h,(p[0].clamp(6.0,24.0)*cx.s).round().max(1.0) as usize,charset,color,bg); return; }
    let src=b.to_vec();
    let amount=p[0].clamp(0.0,1.0);
    for y in 0..h {
        for x in 0..w {
            let i=(y*w+x)*4;
            let original=rgb(&src[i..i+4]);
            let c=match e {
                Effect::Rgbsplit {mode,..} => {
                    let k=p[2].clamp(0.0,1.0)*cx.s;
                    let (rx,ry)=if mode=="radial" { (2.0*x as f64/w as f64-1.0,2.0*y as f64/h as f64-1.0) } else { (1.0,1.0) };
                    let dx=p[0].clamp(-200.0,200.0)*k*rx;
                    let dy=p[1].clamp(-200.0,200.0)*k*ry;
                    [sample(&src,w,h,x as f64-dx,y as f64-dy)[0],original[1],sample(&src,w,h,x as f64+dx,y as f64+dy)[2]]
                }
                Effect::Glitch {seed,..} => {
                    let event=(p[TIME]*p[1].clamp(0.0,120.0)).floor() as i32;
                    let sd=*seed as u32 ^ (*seed>>32) as u32 ^ hash(event,0,91);
                    let block=(p[2].clamp(1.0,1024.0)*cx.s).max(1.0);
                    let lx=x as i32+cx.ox-cx.ax;
                    let ly=y as i32+cx.oy-cx.ay;
                    let n=hash((lx as f64/block).floor() as i32,(ly as f64/block).floor() as i32,sd);
                    let slice=hash(0,(ly as f64/(block*0.3).max(1.0)).floor() as i32,sd);
                    let active=unit(n)<amount as f32*0.55;
                    let shift=if active { (unit(n ^ 0x1234) as f64-0.5)*block*4.0*amount } else if unit(slice)<amount as f32*0.2 { (unit(slice ^ 777) as f64-0.5)*block*6.0*amount } else {0.0};
                    let mut c=sample(&src,w,h,x as f64+shift,y as f64);
                    if active {
                        c[0]=sample(&src,w,h,x as f64+shift+8.0*amount*cx.s,y as f64)[0];
                        if n%5==0 { c=c.map(|v| (v*3.0).round()/3.0); }
                    }
                    c
                }
                Effect::Vhs {seed,..} => {
                    let sd=*seed as u32 ^ (*seed>>32) as u32;
                    let ly=(y as i32+cx.oy-cx.ay) as f64/cx.s.max(0.001);
                    let t=p[TIME];
                    let shift=((ly*0.075+t*3.1+unit(hash(0,0,sd)) as f64*6.0).sin()*1.8+(ly*0.011-t*1.7).sin()*2.0)*amount*cx.s;
                    let center=sample(&src,w,h,x as f64+shift,y as f64);
                    let lum=luma(center);
                    let mut chroma=[0.0;3];
                    for k in -4..=4 {
                        let q=sample(&src,w,h,x as f64+shift+k as f64*cx.s,y as f64);
                        let l=luma(q);
                        for ch in 0..3 { chroma[ch]+=(q[ch]-l)/9.0; }
                    }
                    let band=((ly-t*45.0).rem_euclid(240.0)-120.0).abs();
                    let noise=(unit(hash(x as i32+cx.ox-cx.ax,ly as i32,sd ^ (t*60.0).floor() as u32))-0.5)*if band<10.0 {0.42} else {0.035};
                    let scan=if ly.floor() as i32%2==0 {0.92} else {1.0};
                    std::array::from_fn(|ch| original[ch]+amount as f32*((lum+chroma[ch]*0.86)*scan+noise-original[ch]))
                }
                _ => original,
            };
            put(&mut b[i..i+4],c);
        }
    }
}

fn bayer(x: i32,y: i32,size:u32)->f32 {
    let mut v=0;
    for bit in 0..size.trailing_zeros() { let a=(x>>bit)&1; let b=(y>>bit)&1; v=v*4+((a^b)*2+b); }
    (v as f32+0.5)/(size*size) as f32-0.5
}
// Fixed progressive farthest-point toroidal rank tile. Repulsive placement suppresses
// low spatial frequencies; computed once, independent of frames or worker scheduling.
fn blue()-> &'static [f32] {
    static TILE: std::sync::OnceLock<Vec<f32>>=std::sync::OnceLock::new();
    TILE.get_or_init(|| {
        let n=32usize; let mut dist=vec![usize::MAX;n*n]; let mut ranks=vec![0.0;n*n]; let mut used=vec![false;n*n]; let mut next=17;
        for rank in 0..n*n {
            used[next]=true; ranks[next]=(rank as f32+0.5)/(n*n) as f32-0.5;
            let (x,y)=(next%n,next/n);
            for j in 0..n*n { let dx=x.abs_diff(j%n).min(n-x.abs_diff(j%n)); let dy=y.abs_diff(j/n).min(n-y.abs_diff(j/n)); dist[j]=dist[j].min(dx*dx+dy*dy); }
            next=(0..n*n).filter(|&j| !used[j]).max_by_key(|&j| (dist[j],hash(j as i32,0,42))).unwrap_or(0);
        }
        ranks
    })
}
fn quant(c:[f32;3],levels:f32,palette:Option<&[[f64;4]]>)->[f32;3] {
    if let Some(p)=palette {
        let q=p.iter().min_by(|a,b| {
            let d=|v:&&[f64;4]| (0..3).map(|i| (c[i]-v[i] as f32).powi(2)).sum::<f32>();
            d(a).total_cmp(&d(b))
        }).unwrap();
        [q[0] as f32,q[1] as f32,q[2] as f32]
    } else { c.map(|v| (v.clamp(0.0,1.0)*(levels-1.0)).round()/(levels-1.0)) }
}
fn dither(b:&mut [u8],w:usize,h:usize,levels:f64,mode:&str,size:u32,palette:Option<&[[f64;4]]>,cx:&FxCtx) {
    let levels=levels.round().clamp(2.0,256.0) as f32;
    let mut err=vec![[0.0f32;3];w+2]; let mut next=err.clone();
    for y in 0..h {
        for x in 0..w {
            let i=(y*w+x)*4;
            if b[i+3]==0 { continue; }
            let c=rgb(&b[i..i+4]);
            let lx=x as i32+cx.ox-cx.ax; let ly=y as i32+cx.oy-cx.ay;
            let n=if mode=="bluenoise" {blue()[ly.rem_euclid(32) as usize*32+lx.rem_euclid(32) as usize]} else {bayer(lx,ly,size)};
            let adjusted=std::array::from_fn(|ch| c[ch]+if mode=="floyd" {err[x+1][ch]} else {n/if palette.is_some() {2.0} else {levels-1.0}});
            let q=quant(adjusted,levels,palette);
            put(&mut b[i..i+4],q);
            if mode=="floyd" { for ch in 0..3 {
                let d=(adjusted[ch]-q[ch])*b[i+3] as f32/255.0;
                err[x+2][ch]+=d*7.0/16.0; next[x][ch]+=d*3.0/16.0; next[x+1][ch]+=d*5.0/16.0; next[x+2][ch]+=d/16.0;
            }}
        }
        std::mem::swap(&mut err,&mut next); next.fill([0.0;3]);
    }
}
fn glyph(c:char)->[u8;7] {
    match c {
        '.'=>[0,0,0,0,0,6,6], ':'=>[0,6,6,0,6,6,0], '-'=>[0,0,0,31,0,0,0],
        '='=>[0,0,31,0,31,0,0], '+'=>[0,4,4,31,4,4,0], '*'=>[0,21,14,31,14,21,0],
        '#'=>[10,31,10,10,31,10,0], '%'=>[25,26,2,4,8,11,19], '@'=>[14,17,23,21,23,16,15], _=>[0;7]
    }
}
fn ascii(b:&mut [u8],w:usize,h:usize,cell:usize,charset:&str,color:&str,bg:&[f64;4]) {
    let chars:Vec<_>=charset.chars().collect();
    for y0 in (0..h).step_by(cell) { for x0 in (0..w).step_by(cell) {
        let mut sum=[0.0;3]; let mut weight=0.0;
        for y in y0..(y0+cell).min(h) {for x in x0..(x0+cell).min(w) {let p=&b[(y*w+x)*4..][..4]; weight+=p[3] as f32; for ch in 0..3 {sum[ch]+=p[ch] as f32;}}}
        let c=if weight>0.0 {sum.map(|v|v/weight)} else {[0.0;3]};
        let g=glyph(chars[(luma(c)*(chars.len()-1) as f32).round() as usize]);
        for y in y0..(y0+cell).min(h) {for x in x0..(x0+cell).min(w) {
            let gx=(x-x0)*6/cell; let gy=(y-y0)*8/cell;
            let on=gx<5 && gy<7 && g[gy]&(1<<(4-gx))!=0;
            let p=&mut b[(y*w+x)*4..][..4];
            if on {put(p,if color=="source" {c} else {[1.0;3]});} else {p[3]=(p[3] as f64*bg[3]).round() as u8; put(p,[bg[0] as f32,bg[1] as f32,bg[2] as f32]);}
        }}
    }}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn effects()->Vec<Effect> {
        [r#"{"type":"rgbsplit","dx":5,"dy":2,"amount":1}"#,
         r#"{"type":"rgbsplit","dx":5,"dy":2,"amount":1,"mode":"radial"}"#,
         r#"{"type":"glitch","amount":1,"seed":42,"rate":8,"block":12}"#,
         r#"{"type":"vhs","amount":1,"seed":42}"#,
         r#"{"type":"dither","levels":3,"mode":"bayer","size":2}"#,
         r#"{"type":"dither","levels":3,"mode":"bayer","size":8}"#,
         r#"{"type":"dither","levels":2,"mode":"bluenoise"}"#,
         r#"{"type":"dither","levels":2,"mode":"floyd"}"#,
         r#"{"type":"dither","levels":2,"mode":"floyd","palette":[[0,0,0,1],[1,1,1,1]]}"#,
         r#"{"type":"ascii","cell":8,"charset":" .:-=+*#%@","color":"source","bg":[0,0,0,1]}"#,
         r#"{"type":"ascii","cell":6,"charset":" .#@","color":"mono","bg":[0,0,0,0]}"#].iter().map(|s|serde_json::from_str(s).unwrap()).collect()
    }
    fn source(w:usize,h:usize)->Vec<u8> {
        (0..w*h).flat_map(|i| {let a=(hash(i as i32,0,7)%256) as u8; [((i*3)%256) as u8,((i*7)%256) as u8,((i*11)%256) as u8,a].map(|v|v.min(a))}).collect()
    }
    fn run(e:&Effect, b:&[u8],w:usize,h:usize,t:f64,zero:bool)->Vec<u8> {
        let mut p=[0.0;NPARAM]; for (i,f) in e.look_fields().iter().enumerate() {p[i]=e.look_field(f);if zero && *f=="amount" {p[i]=0.0;}}
        p[TIME]=t; let mut b=b.to_vec();
        apply(e,&p,&mut b,w,h,&FxCtx{ox:3,oy:7,ax:0,ay:0,s:1.0,frame:(t*30.0) as u32}); b
    }
    #[test]
    fn determinism_identity_bounds_and_static_time() {
        for (w,h) in [(1,1),(7,11),(65,47)] { let src=source(w,h);
            for e in effects() {
                let a=run(&e,&src,w,h,0.0,false);
                assert_eq!(a,run(&e,&src,w,h,0.0,false),"{e:?}");
                assert!(a.chunks_exact(4).all(|p|p[..3].iter().all(|c|*c<=p[3])),"{e:?}");
                if !e.time_dependent() {assert_eq!(a,run(&e,&src,w,h,7.0,false),"{e:?}");}
                if matches!(e,Effect::Rgbsplit{..}|Effect::Glitch{..}|Effect::Vhs{..}) {assert_eq!(src,run(&e,&src,w,h,0.0,true));}
                if w>7 {assert_ne!(src,a,"{e:?} changes input");}
            }
        }
    }
    #[test]
    fn temporal_events_and_seed() {
        let src=source(64,64);
        for e in effects().into_iter().filter(|e|e.time_dependent()) {
            assert_ne!(run(&e,&src,64,64,0.0,false),run(&e,&src,64,64,1.0,false));
            let mut other=e.clone(); match &mut other {Effect::Glitch{seed,..}|Effect::Vhs{seed,..}=>*seed+=1,_=>{}}
            assert_ne!(run(&e,&src,64,64,0.0,false),run(&other,&src,64,64,0.0,false));
            if matches!(e,Effect::Glitch{..}) {assert_eq!(run(&e,&src,64,64,0.0,false),run(&e,&src,64,64,0.01,false));}
        }
    }
    #[test]
    fn bayer_is_a_permutation_and_palette_is_exact() {
        for size in [2,4,8] {
            let mut v:Vec<_>=(0..size).flat_map(|y|(0..size).map(move |x|((bayer(x as i32,y as i32,size)+0.5)*(size*size) as f32) as u32)).collect();
            v.sort(); assert_eq!(v,(0..size*size).collect::<Vec<_>>());
        }
        let e=effects().remove(8);let mut src=source(65,47);src.chunks_exact_mut(4).for_each(|p|p[3]=255);
        let out=run(&e,&src,65,47,0.0,false);
        assert!(out.chunks_exact(4).all(|p|p==[0,0,0,255] || p==[255,255,255,255]));
    }
    #[test]
    fn evaluated_tracks_and_clock_signatures() {
        for e in effects() {
            let mut scene: agentfx_ir::Scene = serde_json::from_value(serde_json::json!({
                "version":1,"width":32,"height":32,"fps":30,"frames":30,
                "nodes":[{"name":"root"}]
            })).unwrap();
            scene.nodes[0].effects.push(e.clone());
            for (slot, field) in e.look_fields().iter().enumerate() {
                scene.tracks = vec![serde_json::from_value(serde_json::json!({
                    "node":0,"property":format!("effects.0.{field}"),"delay":0,
                    "duration":1,"fill":"both","easing":"linear",
                    "keyframes":[{"offset":0,"value":{"number":2}}, {"offset":1,"value":{"number":4}}]
                })).unwrap()];
                let p = crate::schedule::effect_params(&scene, &[0], 0, &e, 0.5);
                assert_eq!(p[slot], 3.0);
                assert_eq!(p[TIME], if e.time_dependent() {0.5} else {0.0});
            }
            let a = crate::schedule::effect_params(&scene, &[], 0, &e, 0.0);
            let b = crate::schedule::effect_params(&scene, &[], 0, &e, 0.5);
            assert_eq!(a == b, !e.time_dependent());
        }
    }
    #[test]
    #[ignore = "manual 1080p cost measurement"]
    fn benchmark_1080p() {
        let src=source(1920,1080);
        for e in effects() {
            let _=run(&e,&src,1920,1080,0.0,false);
            let start=std::time::Instant::now();
            for i in 0..3 {std::hint::black_box(run(&e,&src,1920,1080,i as f64,false));}
            println!("{e:?}: {:.2} ms",start.elapsed().as_secs_f64()*1000.0/3.0);
        }
    }
}
