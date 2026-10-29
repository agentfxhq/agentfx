//! Painter side of the fluid layer (X5, see `fluid.rs`): source nodes rasterized as dye
//! masks at grid resolution, coverage upsampled into the context. Child module of `vraster`.
use super::*;

impl<'a> Painter<'a> {
    /// Fluid content: the cached coverage at the shown simulation time, upsampled (Mitchell)
    /// into a premultiplied buffer covering its ink within the context, drawn 1:1.
    pub(super) fn draw_fluid(&mut self, ctx: &mut RenderContext, view: &View, n: NodeId, fl: &crate::fluid::Fluid, chain: Affine) -> Result<()> {
        let s = fluid_time(self.scene, &self.info.tracks[n], fl, view.t);
        let Some(p) = fl.frame_pos(s) else { return Ok(()) };
        let dev = transform_bbox(&ra(chain), fl.c.rect);
        let (cw, ch) = (ctx.width() as f64, ctx.height() as f64);
        let x0 = dev[0].floor().max(0.0);
        let y0 = dev[1].floor().max(0.0);
        let x1 = (dev[0] + dev[2]).ceil().min(cw);
        let y1 = (dev[1] + dev[3]).ceil().min(ch);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let cov = fl.coverage(p, &mut |_, src, tau| self.fluid_mask(n, fl, src, tau))?;
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut px = Pixmap::new(w as u16, h as u16);
        let inv = chain.inverse().as_coeffs();
        crate::fluid::paint(&cov, fl.gw, fl.gh, &fl.c, inv, x0, y0, px.data_as_u8_slice_mut(), w, h, self.fluid_matte > 0);
        ctx.set_transform(Affine::IDENTITY);
        draw_pixmap(ctx, Arc::new(px), x0, y0);
        Ok(())
    }

    /// Coverage (0..1, grid resolution) of source node `src`'s subtree at simulation time
    /// `tau`, in fluid node `n`'s grid. Evaluated from the tracks at that time (not from the
    /// schedule: a cut-out frame range simulates from before its start); the source's own
    /// opacity and its ancestors' are ignored (a dissolving title fades while it pours in).
    fn fluid_mask(&mut self, n: NodeId, fl: &crate::fluid::Fluid, src: NodeId, tau: f64) -> Result<Vec<f32>> {
        let scene = self.scene;
        let ts = tau - fl.c.time_offset;
        let base_m: Vec<agentfx_ir::Affine> = scene.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect();
        let mut locals = schedule::eval_locals(scene, &self.sched.tracks, &base_m, ts)?;
        let mut a = Some(src);
        while let Some(x) = a {
            locals[x].1 = 1.0;
            a = self.sched.parents[x];
        }
        let states = schedule::chains(&self.sched.preorder, &self.sched.parents, &locals);
        let frame = ((ts * scene.fps as f64).round().max(0.0) as usize).min(self.sched.frames.saturating_sub(1));
        let content: Vec<Option<usize>> = (0..scene.nodes.len()).map(|x| schedule::seq_state(scene, x, frame)).collect();
        let view = View { t: ts, states: &states, content: &content };
        let res = fl.c.res.clamp(0.05, 1.0);
        let g = Affine::scale(res) * Affine::translate((-fl.c.rect[0], -fl.c.rect[1])) * k(&states[n].chain).inverse();
        let (gw, gh) = (fl.gw, fl.gh);
        let mut inside = vec![false; scene.nodes.len()];
        inside[src] = true;
        let mut ctx = self.take_ctx(gw as u16, gh as u16, 0);
        self.iso += 1;
        let mut r = Ok(());
        for &x in &self.sched.preorder {
            if x != src {
                match self.sched.parents[x] {
                    Some(p) if inside[p] && !scene.nodes[x].hidden => inside[x] = true,
                    _ => continue,
                }
            }
            let o = states[x].eff_opacity;
            if o <= 0.0 {
                continue;
            }
            r = self.draw_content(&mut ctx, &view, x, g * k(&states[x].chain), o, [0.0, 0.0, gw as f64, gh as f64]);
            if r.is_err() {
                break;
            }
        }
        self.iso -= 1;
        ctx.flush();
        let mut px = Pixmap::new(gw as u16, gh as u16);
        ctx.render(&mut px, &mut self.res);
        self.give_ctx(ctx);
        r?;
        Ok(px.data_as_u8_slice().chunks_exact(4).map(|p| p[3] as f32 / 255.0).collect())
    }
}
