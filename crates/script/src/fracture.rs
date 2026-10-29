//! Seeded Voronoi tessellation by exact half-plane clipping of the source rectangle.
pub type Point = [f64; 2];
pub fn cells(w: f64, h: f64, count: usize, seed: u64) -> Vec<Vec<Point>> {
    let mut state = seed;
    let mut random = || { state = state.wrapping_add(0x9e3779b97f4a7c15); let mut z = state; z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64 };
    let sites: Vec<Point> = (0..count).map(|_| [random()*w, random()*h]).collect();
    sites.iter().enumerate().map(|(i,a)| {
        let mut poly = vec![[0.,0.],[w,0.],[w,h],[0.,h]];
        for (j,b) in sites.iter().enumerate() { if i == j { continue; }
            let n = [b[0]-a[0], b[1]-a[1]];
            let c = (b[0]*b[0]+b[1]*b[1]-a[0]*a[0]-a[1]*a[1])/2.;
            let mut out = vec![];
            for k in 0..poly.len() { let p=poly[k]; let q=poly[(k+1)%poly.len()];
                let dp=p[0]*n[0]+p[1]*n[1]-c; let dq=q[0]*n[0]+q[1]*n[1]-c;
                if dp <= 0. { out.push(p); }
                if (dp <= 0.) != (dq <= 0.) { let t=dp/(dp-dq); out.push([p[0]+t*(q[0]-p[0]),p[1]+t*(q[1]-p[1])]); }
            } poly=out;
        } poly
    }).collect()
}
pub fn area(p: &[Point]) -> f64 { (0..p.len()).map(|i| {let q=p[(i+1)%p.len()]; p[i][0]*q[1]-q[0]*p[i][1]}).sum::<f64>().abs()/2. }
#[cfg(test)] mod tests { use super::*; #[test] fn coverage_and_seed() { for seed in 0..20 { let p=cells(1920.,1080.,24,seed); assert_eq!(p,cells(1920.,1080.,24,seed)); assert!((p.iter().map(|p|area(p)).sum::<f64>()/(1920.*1080.)-1.).abs()<0.005); } } }
