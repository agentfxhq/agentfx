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
/// Sampling tables of rgbsplit / glitch / vhs, shared by the CPU loop below and the GPU
/// kernel (`gpu/digital.wgsl`), so both sample exactly the same pixels: every f64 position
/// (per row, per column or per glitch block) is computed here once. A position `x + d`
/// rounds as `x + floor(d + 0.5)` (the same as `(x + d).round()` for integer x, also
/// after clamping to the buffer, up to f64 rounding of `x + d` within ~1e-12 of a half).
pub(crate) struct Tab {
    /// 0 rgbsplit, 1 glitch, 2 vhs
    pub mode: u32,
    /// rgbsplit: absolute clamped sample columns / rows: red x [0, w), blue x [w, 2w),
    /// red y [2w, 2w + h), blue y [2w + h, 2w + 2h).
    /// glitch: block column per x [0, w), block row per y [w, w + h), the row's slice
    /// offset [w + h, w + 2h), then per block (`gw` per row) 3 words: flags (1 active,
    /// 2 posterize), offset, red offset.
    /// vhs: 12 words per row: 9 tap offsets (k = -4..=4), the noise row, scan, noise
    /// amplitude (f32 bits).
    pub t: Vec<i32>,
    pub gw: usize,
    /// vhs: noise column origin and seed
    pub x0: i32,
    pub seed: u32,
}

fn half_up(d: f64) -> i32 {
    (d + 0.5).floor().clamp(-1e9, 1e9) as i32
}

impl Tab {
    /// None: not one of the three (or a no-op `amount`).
    pub(crate) fn new(e: &Effect, p: &[f64; NPARAM], w: usize, h: usize, cx: &FxCtx) -> Option<Tab> {
        if w == 0 || h == 0 || matches!(e, Effect::Rgbsplit{..}) && p[2]<=0.0 || matches!(e, Effect::Glitch{..}|Effect::Vhs{..}) && p[0]<=0.0 { return None; }
        let amount=p[0].clamp(0.0,1.0);
        let clampi=|v: f64, n: usize| v.round().clamp(0.0,(n-1) as f64) as i32;
        match e {
            Effect::Rgbsplit {mode,..} => {
                let k=p[2].clamp(0.0,1.0)*cx.s;
                let radial=mode=="radial";
                let dx=|x: usize| p[0].clamp(-200.0,200.0)*k*if radial {2.0*x as f64/w as f64-1.0} else {1.0};
                let dy=|y: usize| p[1].clamp(-200.0,200.0)*k*if radial {2.0*y as f64/h as f64-1.0} else {1.0};
                let mut t=Vec::with_capacity(2*w+2*h);
                t.extend((0..w).map(|x| clampi(x as f64-dx(x),w)));
                t.extend((0..w).map(|x| clampi(x as f64+dx(x),w)));
                t.extend((0..h).map(|y| clampi(y as f64-dy(y),h)));
                t.extend((0..h).map(|y| clampi(y as f64+dy(y),h)));
                Some(Tab {mode:0,t,gw:0,x0:0,seed:0})
            }
            Effect::Glitch {seed,..} => {
                let event=(p[TIME]*p[1].clamp(0.0,120.0)).floor() as i32;
                let sd=*seed as u32 ^ (*seed>>32) as u32 ^ hash(event,0,91);
                let block=(p[2].clamp(1.0,1024.0)*cx.s).max(1.0);
                let bx: Vec<i32>=(0..w).map(|x| ((x as i32+cx.ox-cx.ax) as f64/block).floor() as i32).collect();
                let by: Vec<i32>=(0..h).map(|y| ((y as i32+cx.oy-cx.ay) as f64/block).floor() as i32).collect();
                let (gw,gh)=((bx[w-1]-bx[0]+1) as usize,(by[h-1]-by[0]+1) as usize);
                let mut t=Vec::with_capacity(w+2*h+3*gw*gh);
                t.extend(bx.iter().map(|v| v-bx[0]));
                t.extend(by.iter().map(|v| v-by[0]));
                t.extend((0..h).map(|y| {
                    let ly=y as i32+cx.oy-cx.ay;
                    let slice=hash(0,(ly as f64/(block*0.3).max(1.0)).floor() as i32,sd);
                    if unit(slice)<amount as f32*0.2 { half_up((unit(slice ^ 777) as f64-0.5)*block*6.0*amount) } else {0}
                }));
                // (one block per pixel at the smallest block size: rows of blocks in parallel)
                use rayon::prelude::*;
                let base=t.len();
                t.resize(base+3*gw*gh,0);
                t[base..].par_chunks_mut(3*gw).enumerate().for_each(|(j,row)| for (i,b) in row.chunks_exact_mut(3).enumerate() {
                    let n=hash(bx[0]+i as i32,by[0]+j as i32,sd);
                    if unit(n)<amount as f32*0.55 {
                        let shift=(unit(n ^ 0x1234) as f64-0.5)*block*4.0*amount;
                        b.copy_from_slice(&[1|if n%5==0 {2} else {0},half_up(shift),half_up(shift+8.0*amount*cx.s)]);
                    }
                });
                Some(Tab {mode:1,t,gw,x0:0,seed:0})
            }
            Effect::Vhs {seed,..} => {
                let sd=*seed as u32 ^ (*seed>>32) as u32;
                let tm=p[TIME];
                let mut t=Vec::with_capacity(12*h);
                for y in 0..h {
                    let ly=(y as i32+cx.oy-cx.ay) as f64/cx.s.max(0.001);
                    let shift=((ly*0.075+tm*3.1+unit(hash(0,0,sd)) as f64*6.0).sin()*1.8+(ly*0.011-tm*1.7).sin()*2.0)*amount*cx.s;
                    t.extend((-4..=4).map(|k| half_up(shift+k as f64*cx.s)));
                    let band=((ly-tm*45.0).rem_euclid(240.0)-120.0).abs();
                    let scan: f32=if ly.floor() as i32%2==0 {0.92} else {1.0};
                    let amp: f32=if band<10.0 {0.42} else {0.035};
                    t.extend([ly as i32,scan.to_bits() as i32,amp.to_bits() as i32]);
                }
                Some(Tab {mode:2,t,gw:0,x0:cx.ox-cx.ax,seed:sd ^ (tm*60.0).floor() as u32})
            }
            _ => None,
        }
    }
}

pub fn apply(e: &Effect, p: &[f64; NPARAM], b: &mut [u8], w: usize, h: usize, cx: &FxCtx) {
    if w==0 || h==0 { return; }
    if let Effect::Dither {mode,size,palette,..}=e { dither(b,w,h,p[0],mode,*size,palette.as_deref(),cx); return; }
    if let Effect::Ascii {charset,color,bg,..}=e { ascii(b,w,h,(p[0].clamp(6.0,24.0)*cx.s).round().max(1.0) as usize,charset,color,bg); return; }
    let Some(tab)=Tab::new(e,p,w,h,cx) else { return };
    // rows in parallel: every output pixel reads only `src` (bitwise the serial result);
    // the source is unpremultiplied once, not per sample
    use rayon::prelude::*;
    let src: Vec<[f32; 3]>=b.par_chunks(4).map(rgb).collect();
    let amount=p[0].clamp(0.0,1.0) as f32;
    let t=&tab.t;
    b.par_chunks_mut(w*4).enumerate().for_each(|(y,row)| {
        let line=&src[y*w..(y+1)*w];
        let at=|x: usize, d: i32| line[(x as i64+d as i64).clamp(0,w as i64-1) as usize];
        for x in 0..w {
            let original=line[x];
            let c=match tab.mode {
                0 => [src[t[2*w+y] as usize*w+t[x] as usize][0],original[1],src[t[2*w+h+y] as usize*w+t[w+x] as usize][2]],
                1 => {
                    let k=w+2*h+3*(t[w+y] as usize*tab.gw+t[x] as usize);
                    if t[k]&1!=0 {
                        let mut c=at(x,t[k+1]);
                        c[0]=at(x,t[k+2])[0];
                        if t[k]&2!=0 { c=c.map(|v| (v*3.0).round()/3.0); }
                        c
                    } else { at(x,t[w+h+y]) }
                }
                _ => {
                    let r=&t[12*y..12*y+12];
                    let lum=luma(at(x,r[4]));
                    let mut chroma=[0.0;3];
                    for &d in &r[..9] {
                        let q=at(x,d);
                        let l=luma(q);
                        for ch in 0..3 { chroma[ch]+=(q[ch]-l)/9.0; }
                    }
                    let noise=(unit(hash(x as i32+tab.x0,r[9],tab.seed))-0.5)*f32::from_bits(r[11] as u32);
                    let scan=f32::from_bits(r[10] as u32);
                    std::array::from_fn(|ch| original[ch]+amount*((lum+chroma[ch]*0.86)*scan+noise-original[ch]))
                }
            };
            put(&mut row[x*4..x*4+4],c);
        }
    });
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
    /// The per-pixel evaluation before the sampling tables (`Tab`), for the comparison below.
    fn reference(e:&Effect,p:&[f64;NPARAM],b:&mut [u8],w:usize,h:usize,cx:&FxCtx) {
        fn sample(b:&[[f32;3]],w:usize,h:usize,x:f64,y:f64)->[f32;3] {
            let x=x.round().clamp(0.0,(w-1) as f64) as usize;
            let y=y.round().clamp(0.0,(h-1) as f64) as usize;
            b[y*w+x]
        }
        if matches!(e, Effect::Rgbsplit{..}) && p[2]<=0.0 || matches!(e, Effect::Glitch{..}|Effect::Vhs{..}) && p[0]<=0.0 { return; }
        let src: Vec<[f32; 3]>=b.chunks(4).map(rgb).collect();
        let amount=p[0].clamp(0.0,1.0);
        for y in 0..h { for x in 0..w {
            let original=src[y*w+x];
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
            put(&mut b[(y*w+x)*4..(y*w+x)*4+4],c);
        }}
    }
    #[test]
    fn sampling_tables_are_bitwise_the_direct_evaluation() {
        let looks=[r#"{"type":"rgbsplit","dx":5,"dy":2,"amount":1}"#, r#"{"type":"rgbsplit","dx":-17.3,"dy":4.1,"amount":0.7,"mode":"radial"}"#,
            r#"{"type":"glitch","amount":1,"seed":42,"rate":8,"block":12}"#, r#"{"type":"glitch","amount":0.6,"seed":13,"rate":14,"block":2.5}"#,
            r#"{"type":"vhs","amount":1,"seed":42}"#, r#"{"type":"vhs","amount":0.05,"seed":3}"#];
        let (w,h)=(97,61); let src=source(w,h);
        for s in looks { let e:Effect=serde_json::from_str(s).unwrap();
            for (sc,t,ox) in [(1.0,0.0,3),(1.37,2.5,-41),(0.5,13.1,700)] {
                let mut p=[0.0;NPARAM]; for (i,f) in e.look_fields().iter().enumerate() {p[i]=e.look_field(f);} p[TIME]=t;
                let cx=FxCtx{ox,oy:-19,ax:5,ay:11,s:sc,frame:3};
                let (mut a,mut b)=(src.clone(),src.clone());
                apply(&e,&p,&mut a,w,h,&cx); reference(&e,&p,&mut b,w,h,&cx);
                assert!(a==b,"{s} s {sc} t {t}");
            }
        }
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
