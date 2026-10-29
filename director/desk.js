"use strict";
/* Director's desk v3: projects home, switcher, command palette; per project a stream of decision
   cards the agent posts (stream.js), a state rail, and the v2 stage pages as panels.
   No build, no dependencies. State comes from desk.py's JSON API; every choice POSTs one action
   into the project's feedback.jsonl. */

const STAGES = [
  { id: "mode", n: 0, name: "Mode" },
  { id: "brief", n: 1, name: "Brief" },
  { id: "directions", n: 2, name: "Directions" },
  { id: "dramaturgy", n: 3, name: "Dramaturgy" },
  { id: "storyboard", n: 4, name: "Storyboard" },
  { id: "variants", n: 5, name: "Hero variants" },
  { id: "render", n: 6, name: "Render" },
];
const STAGE_IDS = STAGES.map(s => s.id);
const APPROVAL_TARGET = { brief: "brief", directions: "direction", dramaturgy: "dramaturgy", storyboard: "storyboard" };
const PSTATE_LABEL = { done: "done", current: "in progress", waiting: "your turn", stale: "approval stale", todo: "" };
const BADGE_LABEL = { waiting: "Waiting for you", working: "Agent working", pending: n => `${n} sent, agent has not read ${n === 1 ? "it" : "them"} yet`, cards: n => `${n} open card${n === 1 ? "" : "s"}` };
const RAIL = STAGES.filter(s => s.id !== "mode");
const STATUS_ORDER = ["active", "paused", "done", "archived"];

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const esc = s => String(s ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmt = t => (Math.max(0, t)).toFixed(2) + " s";
const mmss = t => { t = Math.max(0, t); const m = Math.floor(t / 60); const s = (t - m * 60).toFixed(2).padStart(5, "0"); return `${m}:${s}`; };
const isMac = /Mac|iPhone|iPad/.test(navigator.platform);
const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");
const phone = matchMedia("(max-width: 760px)");

/* ---------- app state ---------- */
let P = [], homeV = "", single = false;          // projects
let route = { view: "home" };                   // { view: home|project|missing, slug, stage }
let slug = null, S = null, D = null, version = "", mediaV = "", selected = null, keys = {};
let ui = freshUi();
const home = { filter: localStorage.getItem("desk-filter") || "active", q: "", sort: localStorage.getItem("desk-sort") || "updated" };
function freshUi() { return { order: null, focus: null, dfocus: null, align: true, solo: null, T: 0, playing: false, picked: new Set() }; }

/* ---------- theme, toast, helpers ---------- */
const root = document.documentElement;
$("#theme").onclick = toggleTheme;
function toggleTheme() { root.dataset.theme = root.dataset.theme === "light" ? "dark" : "light"; localStorage.setItem("desk-theme", root.dataset.theme); }
if (isMac) { $("#mod-key").textContent = "⌘"; $$(".modk").forEach(k => k.textContent = "⌘"); $(".sw-kbd").textContent = "⌘K"; }

function toast(msg) { const t = $("#toast"); t.textContent = msg; t.classList.add("on"); clearTimeout(toast.h); toast.h = setTimeout(() => t.classList.remove("on"), 2400); }
const fileUrl = (sl, p, v = "") => p ? `/files/${encodeURIComponent(sl)}/${p.split("/").map(encodeURIComponent).join("/")}${v ? "?v=" + v : ""}` : "";
const media = p => fileUrl(slug, p, mediaV);
function ago(iso) {
  const t = Date.parse(iso); if (!t) return "";
  const s = (Date.now() - t) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 172800) return "yesterday";
  if (s < 604800) return `${Math.floor(s / 86400)} days ago`;
  return new Date(t).toLocaleDateString("en-GB", { day: "numeric", month: "short" });
}
const stageName = id => STAGES.find(s => s.id === id)?.name || id;
const badgeHtml = (c, plain) => c.badge ? `<span class="badge ${c.badge}${plain ? " plain" : ""}">${esc(c.badge === "pending" ? BADGE_LABEL.pending(c.pending) : c.badge === "cards" ? BADGE_LABEL.cards(c.open_cards) : BADGE_LABEL[c.badge])}</span>`
  : `<span class="badge quiet${plain ? " plain" : ""}">${esc(c.status[0].toUpperCase() + c.status.slice(1))}</span>`;
async function api(path, body) {
  const r = await fetch(path, body ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) } : {});
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || `HTTP ${r.status}`);
  return j;
}
function setLive(ok) { const l = $("#live"); l.classList.toggle("on", ok); l.classList.toggle("off", !ok); l.title = ok ? "Live: the page follows the agent's changes" : "Offline: retrying"; }

/* ========== v3: the card stream, the synced clip player and selection as context ==========
   Kept in this one file on purpose: a running v2 server serves only desk.css and desk.js. */

const CARD_TYPE_LABEL = { choice: "Choice", compare: "Compare", note: "Notes", approve: "Approve", question: "Question" };
const ICON = {
  play: `<svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor" aria-hidden="true"><path d="M3 1.5v11l9.5-5.5z"/></svg>`,
  pause: `<svg width="14" height="14" viewBox="0 0 14 14" fill="currentColor" aria-hidden="true"><rect x="2" y="1" width="3.5" height="12" rx="1"/><rect x="8.5" y="1" width="3.5" height="12" rx="1"/></svg>`,
  x: `<svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M1.5 1.5l7 7M8.5 1.5l-7 7"/></svg>`,
};
const secs = t => (+t).toFixed(2);
const rangeTxt = (a, b) => `${secs(a)} to ${secs(b)} s`;

/* ---------- Sync: N clips on one transport (the v2 variant explorer, made reusable) ----------
   clips: [{id, tag, src, poster, cue, dur, clip_t0}]. With align on, every clip is offset so the
   cues line up; T is the shared timeline. A range (T0..T1) is marked by dragging the lane under
   the scrub, or with I / O at the playhead. */
class Sync {
  static html(clips, { focus, body, foot, single = false } = {}) {
    const anyCue = clips.some(c => c.cue != null);
    return `<div class="sync${single ? " single" : ""}">
      <div class="vgrid n${clips.length}">${clips.map((c, i) => `
        <article class="card clip ${c.id === focus ? "focus" : ""}" data-clip-card="${esc(c.id)}">
          <div class="screen" data-sfocus="${esc(c.id)}">
            <video muted playsinline preload="metadata" data-clip="${esc(c.id)}" src="${esc(c.src)}" ${c.poster ? `poster="${esc(c.poster)}"` : ""}></video>
            <div class="ov">${single ? "" : `<span class="tag">${esc(c.tag || "ABCD"[i])}</span>`}${!single && i < 4 ? `<span class="screen-key" aria-hidden="true">${i + 1}</span>` : ""}<span class="wait hidden">waiting</span>
              <div class="mini"><i></i>${c.cue != null && c.dur ? `<b style="left:${(c.cue / c.dur * 100).toFixed(2)}%"></b>` : ""}</div></div>
          </div>
          ${body ? body(c, i) : ""}
          ${foot ? `<div class="foot">${foot(c, i)}${single ? "" : `<button class="btn small solo" data-ssolo="${esc(c.id)}" style="margin-left:auto" aria-pressed="false" title="Hear only this clip">Solo</button>`}</div>` : ""}
        </article>`).join("")}
      </div>
      <div class="card transport">
        <button class="btn primary play" data-splay aria-label="Play or pause (Space)">${ICON.play}</button>
        <span class="mono readout" data-sread>0:00.00</span>
        <div class="scrub">
          <input type="range" data-sscrub min="0" max="1" step="0.01" value="0" aria-label="Scrub">
          <span class="cue" data-scue></span>
          <div class="rlane" data-slane title="Drag to mark a range (or I and O at the playhead)"><span class="rsel" data-srsel hidden></span><span class="rhint">drag to mark a range</span></div>
        </div>
        ${!single && anyCue && clips.length > 1 ? `<div class="seg" data-salign><button data-al="1" class="on">Align to cue</button><button data-al="0">Own start</button></div>` : ""}
        <span class="rbtns"><button class="btn small ghost" data-sin title="Range in at the playhead (I)">In</button><button class="btn small ghost" data-sout title="Range out at the playhead (O)">Out</button><button class="btn small ghost" data-sclear title="Clear the range" hidden>${ICON.x}</button></span>
      </div>
    </div>`;
  }

  constructor(el, clips, o = {}) {
    this.el = el; this.clips = clips.map(c => ({ ...c })); this.align = o.align ?? true;
    this.T = 0; this.playing = false; this.solo = null; this.focus = o.focus || clips[0]?.id; this.range = null;
    this.on = o.on || {};
    this.vids = this.clips.map(c => ({ c, video: $(`video[data-clip="${CSS.escape(c.id)}"]`, el), card: $(`[data-clip-card="${CSS.escape(c.id)}"]`, el) }));
    this.scrub = $("[data-sscrub]", el);
    this.vids.forEach(({ c, video }) => {
      video.muted = true;
      video.addEventListener("loadedmetadata", () => {
        if (!c.dur && isFinite(video.duration)) { c.dur = video.duration; this.layout(); }
        this.seek(this.T);
      });
    });
    if (this.clips.length === 1) { this.solo = this.clips[0].id; this.applySolo(); }
    $("[data-splay]", el).onclick = () => this.toggle();
    this.scrub.oninput = () => { if (this.playing) this.toggle(false); this.T = +this.scrub.value; this.seek(this.T); this.readout(); };
    $$("[data-al]", el).forEach(b => b.onclick = () => {
      this.align = b.dataset.al === "1"; $$("[data-al]", el).forEach(x => x.classList.toggle("on", x === b));
      this.T = 0; this.range = null; this.layout(); this.seek(0); this.drawRange(); this.emitRange();
    });
    $$("[data-sfocus]", el).forEach(s => s.addEventListener("click", () => this.setFocus(s.dataset.sfocus)));
    $$("[data-ssolo]", el).forEach(b => b.onclick = () => { this.solo = this.solo === b.dataset.ssolo ? null : b.dataset.ssolo; this.applySolo(); });
    $("[data-sin]", el).onclick = () => this.mark("in");
    $("[data-sout]", el).onclick = () => this.mark("out");
    $("[data-sclear]", el).onclick = () => { this.range = null; this.drawRange(); this.emitRange(); };
    const lane = $("[data-slane]", el);
    lane.addEventListener("pointerdown", e => {
      e.preventDefault(); lane.setPointerCapture(e.pointerId);
      const at = ev => { const r = lane.getBoundingClientRect(); return Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width)) * this.len(); };
      const a = at(e); this.range = { T0: a, T1: a }; this.drawRange();
      const move = ev => { const b = at(ev); this.range = { T0: Math.min(a, b), T1: Math.max(a, b) }; this.drawRange(); };
      const up = ev => {
        lane.removeEventListener("pointermove", move); lane.removeEventListener("pointerup", up);
        if (this.range.T1 - this.range.T0 < 0.05) { this.range = null; this.T = a; this.seek(a); this.readout(); }
        else { this.T = this.range.T0; this.seek(this.T); this.readout(); }
        this.drawRange(); this.emitRange();
      };
      lane.addEventListener("pointermove", move); lane.addEventListener("pointerup", up);
    });
    this.layout(); this.readout();
  }
  offs() { const maxCue = Math.max(0, ...this.clips.map(c => c.cue || 0)); return Object.fromEntries(this.clips.map(c => [c.id, this.align ? maxCue - (c.cue || 0) : 0])); }
  len() { const o = this.offs(); return Math.max(0.01, ...this.clips.map(c => o[c.id] + (c.dur || 0))); }
  clipT(c, T = this.T) { return Math.min(Math.max(0, T - this.offs()[c.id]), c.dur || 0); }
  focused() { return this.clips.find(c => c.id === this.focus) || this.clips[0]; }
  layout() {
    this.scrub.max = this.len();
    const o = this.offs(), cueT = this.align ? Math.max(0, ...this.clips.map(c => o[c.id] + (c.cue || 0))) : (this.focused().cue || 0);
    const cue = $("[data-scue]", this.el);
    cue.hidden = !this.clips.some(c => c.cue != null);
    cue.style.left = `calc(${(cueT / this.len() * 100).toFixed(2)}% - 1px)`;
    this.vids.forEach(({ c, card }) => { const b = $(".mini b", card); if (b && c.dur && c.cue != null) b.style.left = (c.cue / c.dur * 100).toFixed(2) + "%"; });
  }
  setFocus(id) { this.focus = id; this.vids.forEach(({ c, card }) => card.classList.toggle("focus", c.id === id)); this.readout(); this.layout(); this.on.focus?.(id); }
  applySolo() {
    this.vids.forEach(({ c, video }) => { video.muted = this.solo !== c.id; });
    $$("[data-ssolo]", this.el).forEach(x => { const on = x.dataset.ssolo === this.solo; x.classList.toggle("on", on); x.setAttribute("aria-pressed", on); x.textContent = on ? "Solo on" : "Solo"; });
  }
  mark(which) {
    const t = this.T, r = this.range || { T0: which === "in" ? t : 0, T1: which === "out" ? t : this.len() };
    if (which === "in") { r.T0 = t; if (r.T1 <= t) r.T1 = Math.min(this.len(), t + 1); }
    else { r.T1 = t; if (r.T0 >= t) r.T0 = Math.max(0, t - 1); }
    this.range = r.T1 - r.T0 >= 0.05 ? r : null; this.drawRange(); this.emitRange();
  }
  drawRange() {
    const s = $("[data-srsel]", this.el), L = this.len();
    $("[data-sclear]", this.el).hidden = !this.range;
    $(".rhint", this.el).hidden = !!this.range;
    if (!this.range) { s.hidden = true; return; }
    s.hidden = false; s.style.left = (this.range.T0 / L * 100) + "%"; s.style.width = ((this.range.T1 - this.range.T0) / L * 100) + "%";
  }
  emitRange() { this.on.range?.(this.rangeFor(this.focused())); this.readout(); }
  /* the range in the focused clip's own time (and film time when the clip knows where it sits) */
  rangeFor(c) {
    if (!this.range) return null;
    const t0 = this.clipT(c, this.range.T0), t1 = this.clipT(c, this.range.T1);
    if (t1 - t0 < 0.05) return null;
    const out = { clip: c.id, t0: +t0.toFixed(3), t1: +t1.toFixed(3) };
    if (c.clip_t0 != null) { out.t0_film = +(c.clip_t0 + t0).toFixed(3); out.t1_film = +(c.clip_t0 + t1).toFixed(3); }
    return out;
  }
  /* where a note made now lands: the range if one is marked, else the playhead */
  at() {
    const c = this.focused(), r = this.rangeFor(c);
    if (r) return r;
    const t = this.clipT(c), out = { clip: c.id, t: +t.toFixed(3) };
    if (c.clip_t0 != null) out.t_film = +(c.clip_t0 + t).toFixed(3);
    return out;
  }
  readout() {
    const r = $("[data-sread]", this.el); if (!r) return;
    r.textContent = mmss(this.T);
    if (document.activeElement !== this.scrub) this.scrub.value = this.T;
    this.on.time?.(this.at());
  }
  seek(T) {
    const o = this.offs();
    this.vids.forEach(({ c, video, card }) => {
      const t = this.clipT(c, T);
      if (video.readyState >= 1 && Math.abs(video.currentTime - t) > 0.02) video.currentTime = t;
      this.waitTag(c, card, T, o);
    });
  }
  waitTag(c, card, T, o) {
    const w = $(".wait", card), waiting = c.dur && (T < o[c.id] || T > o[c.id] + c.dur);
    w.classList.toggle("hidden", !waiting);
    w.textContent = T < o[c.id] ? `starts in ${(o[c.id] - T).toFixed(2)} s` : "ended";
    const i = $(".mini i", card); if (i && c.dur) i.style.width = (this.clipT(c, T) / c.dur * 100).toFixed(2) + "%";
  }
  toggle(force) {
    this.playing = force ?? !this.playing;
    const L = this.len(), loopR = this.range;
    if (this.playing && (this.T >= L - 0.01 || (loopR && (this.T < loopR.T0 || this.T >= loopR.T1 - 0.01)))) this.T = loopR ? loopR.T0 : 0;
    $("[data-splay]", this.el).innerHTML = this.playing ? ICON.pause : ICON.play;
    cancelAnimationFrame(this.raf);
    if (!this.playing) { this.vids.forEach(({ video }) => video.pause()); this.seek(this.T); return; }
    let last = performance.now();
    const tick = now => {
      this.T += (now - last) / 1000; last = now;
      const end = this.range ? this.range.T1 : L;
      if (this.T >= end) { this.T = end; this.toggle(false); this.readout(); return; }
      const o = this.offs();
      this.vids.forEach(({ c, video, card }) => {
        const inside = this.T >= o[c.id] && this.T < o[c.id] + (c.dur || 0), t = this.clipT(c);
        if (inside) {
          if (video.paused) { video.currentTime = t; video.play().catch(() => {}); }
          else if (Math.abs(video.currentTime - t) > 0.12) video.currentTime = t;
        } else if (!video.paused) video.pause();
        this.waitTag(c, card, this.T, o);
      });
      this.readout();
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
  }
  stop() { if (this.playing) this.toggle(false); }
  state() { return { T: this.T, range: this.range, align: this.align, focus: this.focus, solo: this.solo }; }
  restore(s) {
    if (!s) return;
    this.align = s.align; $$("[data-al]", this.el).forEach(x => x.classList.toggle("on", (x.dataset.al === "1") === this.align));
    this.range = s.range; this.T = s.T;
    if (this.clips.some(c => c.id === s.focus)) this.setFocus(s.focus);
    if (this.clips.length > 1) { this.solo = s.solo; this.applySolo(); }
    this.layout(); this.seek(this.T); this.drawRange(); this.readout();
  }
}

/* ---------- selection as context ---------- */
let ctx = {};
function ctxEmpty() { return !ctx.beat && !ctx.point && !ctx.range; }
function ctxPayload() { return ctxEmpty() ? undefined : JSON.parse(JSON.stringify(ctx)); }
function setCtx(part) {
  ctx = { ...ctx, ...part };
  for (const k of Object.keys(ctx)) if (ctx[k] == null) delete ctx[k];
  renderCtx();
}
function clearCtx() { ctx = {}; renderCtx(); }
function beatLabel(id) { const b = (S?.beats || []).find(x => x.id === id); return b ? `${b.id} ${b.title}` : id; }
function beatAtFilm(t) { return (S?.beats || []).find(b => t >= b.t0 - 1e-6 && t < b.t0 + b.dur - 1e-6)?.id || null; }
function renderCtx() {
  const el = $("#ctxchip");
  const bits = [];
  if (ctx.beat) bits.push(`<span class="cbit"><span class="label">beat</span>${esc(beatLabel(ctx.beat))}</span>`);
  if (ctx.point) bits.push(`<span class="cbit"><span class="label">point</span>${esc(ctx.point.label || ctx.point.src.split("/").pop())} at ${Math.round(ctx.point.x * 100)}%, ${Math.round(ctx.point.y * 100)}%</span>`);
  if (ctx.range) bits.push(`<span class="cbit"><span class="label">range</span>${rangeTxt(ctx.range.t0, ctx.range.t1)}${ctx.range.label ? " on " + esc(ctx.range.label) : ""}</span>`);
  el.hidden = !bits.length || route.view !== "project";
  document.body.classList.toggle("has-ctx", !el.hidden);
  el.innerHTML = bits.length ? `<span class="ctx-k">Context</span>${bits.join("")}<span class="ctx-note">goes with your next answer</span><button class="ctx-x" data-ctx-clear aria-label="Clear the context (X)" title="Clear (X)">${ICON.x}</button>` : "";
  $$(".pointable").forEach(drawMarker);
  $$("[data-beat-pick]").forEach(b => b.classList.toggle("ctx-on", b.dataset.beatPick === ctx.beat));
}
document.addEventListener("click", e => { if (e.target.closest("[data-ctx-clear]")) clearCtx(); });

/* a still you can point at: click sets context {point, beat?} and draws a marker */
function pointable(src, inner, { beat, label } = {}) {
  return `<span class="pointable" data-psrc="${esc(src)}" ${beat ? `data-pbeat="${esc(beat)}"` : ""} data-plabel="${esc(label || "")}" title="Click to point at something in this frame">${inner}</span>`;
}
function drawMarker(w) {
  $(".pmark", w)?.remove();
  if (ctx.point && ctx.point.src === w.dataset.psrc) {
    const m = document.createElement("i"); m.className = "pmark"; m.style.left = ctx.point.x * 100 + "%"; m.style.top = ctx.point.y * 100 + "%"; w.appendChild(m);
  }
}
document.addEventListener("click", e => {
  const w = e.target.closest(".pointable"); if (!w || e.target.closest("button, a, input")) return;
  const r = w.getBoundingClientRect();
  const x = (e.clientX - r.left) / r.width, y = (e.clientY - r.top) / r.height;
  const part = { point: { src: w.dataset.psrc, x: +x.toFixed(4), y: +y.toFixed(4), label: w.dataset.plabel || undefined } };
  if (w.dataset.pbeat) part.beat = w.dataset.pbeat;
  setCtx(part);
});
document.addEventListener("click", e => {
  const b = e.target.closest("[data-beat-pick]"); if (!b) return;
  setCtx({ beat: ctx.beat === b.dataset.beatPick ? null : b.dataset.beatPick });
});

/* ---------- the stream ---------- */
const drafts = {};          // per card id: { sel, mix: {attr: optId}, mixOn, text, notes: [] }
const players = {};         // per card id: Sync
let activeCard = null;      // keyboard target
const cardKeys = {}, cardV = {};
function draft(id) { return drafts[id] || (drafts[id] = { sel: null, mix: {}, mixOn: false, text: "", notes: [], changes: false }); }
function cardsAll() { return (D && D.cards) || []; }
/* newest first by creation time; cards created in the same second (one batch) keep the agent's order */
function openCards() { return cardsAll().filter(c => c.status === "open").sort((a, b) => (Date.parse(b.created) || 0) - (Date.parse(a.created) || 0)); }
function stopPlayers() { Object.values(players).forEach(p => p.stop()); if (vplayer) vplayer.stop(); }

function renderStream() {
  const open = openCards(), el = $("#stream");
  const agent = D.card.agent || {};
  $("#stream-title").textContent = open.length ? `Your turn: ${open.length} open card${open.length === 1 ? "" : "s"}` : "Your turn";
  $("#stream-sub").textContent = open.length ? "newest first" : "";
  // drop cards that left the open set
  $$(".dcard", el).forEach(n => { if (!open.some(c => c.id === n.dataset.card)) { players[n.dataset.card]?.stop(); delete players[n.dataset.card]; delete cardKeys[n.dataset.card]; n.remove(); } });
  $(".stream-empty", el)?.remove();
  if (!open.length) {
    const cur = S.stage && STAGE_IDS.includes(S.stage) ? S.stage : "brief";
    el.insertAdjacentHTML("beforeend", `<div class="stream-empty"><div class="t">Nothing waiting for you</div>
      <p>${agent.state === "working" ? `The agent is working${agent.note ? ": " + esc(agent.note) : ""}. ` : ""}When it needs a decision it posts a card here: a choice, clips to compare, a request for notes, an approval or a question.</p>
      <div class="acts"><a class="btn" href="${projectPath(slug, cur)}" data-link>Open ${esc(stageName(cur))}</a></div></div>`);
    activeCard = null;
  }
  let prev = null;
  open.forEach(c => {
    const key = JSON.stringify(c) + mediaV;
    let n = $(`.dcard[data-card="${CSS.escape(c.id)}"]`, el);
    if (!n || cardKeys[c.id] !== key) {
      const changed = !!n && cardV[c.id] != null && cardV[c.id] !== c.v;
      if (players[c.id]) { draft(c.id).ps = players[c.id].state(); players[c.id].stop(); delete players[c.id]; }
      const tmp = document.createElement("div"); tmp.innerHTML = cardHtml(c, changed);
      const fresh = tmp.firstElementChild;
      if (n) n.replaceWith(fresh); else if (prev) prev.after(fresh); else el.prepend(fresh);
      n = fresh; cardKeys[c.id] = key; cardV[c.id] = c.v; wireCard(n, c);
    } else if (prev ? prev.nextElementSibling !== n : el.firstElementChild !== n) {
      if (prev) prev.after(n); else el.prepend(n);
    }
    prev = n;
  });
  if (!open.some(c => c.id === activeCard)) activeCard = open[0]?.id || null;
  markActive();
  renderHistory();
  renderSide();
}
function markActive() { $$(".dcard").forEach(n => n.classList.toggle("active", n.dataset.card === activeCard)); }
function setActive(id, scroll) {
  activeCard = id; markActive();
  const n = $(`.dcard[data-card="${CSS.escape(id)}"]`);
  if (n && scroll) n.scrollIntoView({ block: "nearest", behavior: reduceMotion.matches ? "auto" : "smooth" });
}

function cardMeta(c) {
  const bits = [`<span class="dc-type t-${esc(c.type)}"><i aria-hidden="true"></i>${CARD_TYPE_LABEL[c.type]}</span>`];
  if (c.stage) bits.push(`<span class="chip">${esc(stageName(c.stage))}</span>`);
  if (c.beat) bits.push(`<button class="chip beatchip" data-beat-pick="${esc(c.beat)}" title="Use this beat as context">${esc(beatLabel(c.beat))}</button>`);
  bits.push(`<span class="dc-when">${esc(ago(c.updated))}${c.v > 1 && c.status === "open" ? ` · v${c.v}` : ""}</span>`);
  return `<header class="dc-head">${bits.join("")}</header>`;
}
function bodyHtml(t) { return t ? `<div class="dc-body">${t.split(/\n{2,}/).map(p => `<p>${esc(p).replace(/\n/g, "<br>")}</p>`).join("")}</div>` : ""; }
function mediaHtml(c, big) {
  const m = c.media; if (!m) return "";
  if (m.clip && c.type !== "note") return `<div class="dc-media"><video controls muted playsinline preload="metadata" src="${media(m.clip)}" ${m.still ? `poster="${media(m.still)}"` : ""}></video></div>`;
  if (m.still) return `<div class="dc-media${big ? " big" : ""}">${pointable(m.still, `<img src="${media(m.still)}" alt="" loading="lazy">`, { beat: c.beat, label: m.label || c.title })}</div>`;
  return "";
}
function cardHtml(c, changed) {
  const d = draft(c.id);
  let inner = "";
  if (c.type === "choice") inner = choiceHtml(c, d);
  else if (c.type === "compare") inner = compareHtml(c, d);
  else if (c.type === "note") inner = noteHtml(c, d);
  else if (c.type === "approve") inner = approveHtml(c, d);
  else if (c.type === "question") inner = questionHtml(c, d);
  return `<article class="card dcard type-${esc(c.type)}" data-card="${esc(c.id)}" tabindex="-1" aria-label="${esc(CARD_TYPE_LABEL[c.type])}: ${esc(c.title)}">
    ${cardMeta(c)}
    ${changed ? `<div class="dc-updated">The agent updated this card (v${c.v}). Look again before you answer.</div>` : ""}
    <h3 class="dc-title">${esc(c.title)}</h3>
    ${bodyHtml(c.body)}
    ${c.type === "note" ? "" : mediaHtml(c)}
    ${inner}
  </article>`;
}
const keyHint = (k, t) => `<span><kbd>${k}</kbd> ${t}</span>`;

/* choice */
function optLabel(c, id) { return (c.options || []).find(o => o.id === id)?.label || id; }
function choiceHtml(c, d) {
  const visual = c.options.some(o => o.still || o.clip);
  const audio = c.options.some(o => o.audio);
  const attrs = c.attributes || [];
  return `<div class="opts n${c.options.length} ${visual ? "visual" : audio ? "audio" : "text"}" role="radiogroup" aria-label="Options">${c.options.map((o, i) => {
    const planned = o.status === "planned";
    return `<button class="copt ${d.sel === o.id ? "sel" : ""} ${planned ? "planned" : ""}" role="radio" aria-checked="${d.sel === o.id}" data-opt="${esc(o.id)}" ${planned ? "disabled" : ""}>
      ${visual ? `<span class="ostill">${o.still ? `<img src="${media(o.still)}" alt="" loading="lazy">` : ""}${o.clip ? `<video muted loop playsinline preload="none" data-hover src="${media(o.clip)}"></video>` : ""}</span>` : ""}
      <span class="otop"><span class="okey" aria-hidden="true">${i + 1}</span><span class="olabel">${esc(o.label)}</span>${c.default === o.id ? `<span class="chip accent">proposed</span>` : ""}${o.status && o.status !== "implemented" ? `<span class="chip ${planned ? "planned" : "warn"}">${esc(o.status)}</span>` : ""}</span>
      ${o.detail ? `<span class="odetail">${esc(o.detail)}</span>` : ""}
      ${o.audio ? `<span class="oaudio"><audio controls preload="none" src="${media(o.audio)}" data-oaudio></audio></span>` : ""}
      ${o.attrs ? `<span class="oattrs">${Object.entries(o.attrs).map(([k, v]) => `<span><b>${esc(k)}</b> ${esc(v)}</span>`).join("")}</span>` : ""}
    </button>`;
  }).join("")}</div>
  ${attrs.length ? `<details class="mixbox" ${d.mixOn ? "open" : ""}><summary>Mix instead: take each attribute from a different option</summary>
    <div class="mixgrid">${attrs.map(a => `<div class="mrow"><span class="label">${esc(a)}</span><div class="seg">${c.options.filter(o => o.status !== "planned").map(o => `<button data-mixattr="${esc(a)}" data-from="${esc(o.id)}" class="${(d.mix[a] || c.default || c.options[0].id) === o.id ? "on" : ""}">${esc(o.label)}</button>`).join("")}</div></div>`).join("")}</div></details>` : ""}
  <input type="text" class="dc-text" data-note data-ctext value="${esc(d.text)}" placeholder="A word for the agent (optional)">
  <footer class="dc-foot">
    <button class="btn primary" data-send>${d.mixOn ? "Send mix" : d.sel ? `Send: ${esc(optLabel(c, d.sel))}` : "Select an option"}</button>
    ${c.default ? `<button class="btn" data-accept>Accept proposed: ${esc(optLabel(c, c.default))}</button>` : ""}
    <span class="dc-keys">${keyHint(`1-${c.options.length}`, "select")}${keyHint("Enter", d.sel ? "send" : "accept proposed")}</span>
  </footer>`;
}

/* compare */
function compareClips(c) { return c.options.map((o, i) => ({ id: o.id, tag: "ABCD"[i], src: media(o.clip), poster: o.still ? media(o.still) : "", cue: o.cue, dur: o.dur, clip_t0: o.clip_t0, label: o.label })); }
function notesListHtml(c, d) {
  if (!d.notes.length) return `<div class="dnotes empty"></div>`;
  return `<ul class="dnotes">${d.notes.map((n, i) => `<li><span class="mono">${n.clip && c.type === "compare" ? esc(optLabel(c, n.clip)) + " " : ""}${n.t0 != null ? rangeTxt(n.t0, n.t1) : n.t != null ? "@ " + secs(n.t) + " s" : ""}${n.beat ? " · " + esc(n.beat) : ""}</span><span>${esc(n.text)}</span><button class="btn ghost small" data-rmnote="${i}" aria-label="Remove note">${ICON.x}</button></li>`).join("")}</ul>`;
}
function notebarHtml(label = "note at 0.00 s") {
  return `<div class="notebar card"><span class="mono" data-noteat>${label}</span><input type="text" data-note data-nt placeholder="A note at this moment or range, Enter adds it"><button class="btn" data-addnote>Add note</button></div>`;
}
function compareHtml(c, d) {
  const sel = d.sel;
  return Sync.html(compareClips(c), {
    focus: sel || c.default || c.options[0].id,
    body: o => { const op = c.options.find(x => x.id === o.id); return `<div class="body"><h3>${esc(op.label)} ${o.cue != null ? `<span class="mono">cue ${secs(o.cue)} s</span>` : ""}${c.default === op.id ? ` <span class="chip accent">proposed</span>` : ""}</h3>${op.detail ? `<p class="diff">${esc(op.detail)}</p>` : ""}</div>`; },
    foot: o => `<button class="btn small ${sel === o.id ? "primary" : ""}" data-opt="${esc(o.id)}" aria-pressed="${sel === o.id}">${sel === o.id ? "Selected" : "Select"}</button><button class="btn small" data-branch="${esc(o.id)}">Branch</button>`,
  }) + notebarHtml() + notesListHtml(c, d) + `
  <input type="text" class="dc-text" data-ctext value="${esc(d.text)}" placeholder="A word for the agent (optional)">
  <footer class="dc-foot">
    <button class="btn primary" data-send>${sel ? `Send: ${esc(optLabel(c, sel))}${d.notes.length ? ` + ${d.notes.length} note${d.notes.length === 1 ? "" : "s"}` : ""}` : d.notes.length ? `Send ${d.notes.length} note${d.notes.length === 1 ? "" : "s"}` : "Select a clip"}</button>
    ${c.default && !sel ? `<button class="btn" data-accept>Accept proposed: ${esc(optLabel(c, c.default))}</button>` : ""}
    <button class="btn ghost" data-none>None of these</button>
    <span class="dc-keys">${keyHint("Space", "play")}${keyHint("I O", "range")}${keyHint("N", "note")}${keyHint("Enter", "send")}</span>
  </footer>`;
}

/* note */
function noteHtml(c, d) {
  const m = c.media || {};
  const player = m.clip ? Sync.html([{ id: "clip", src: media(m.clip), poster: m.still ? media(m.still) : "", cue: m.cue, dur: m.dur, clip_t0: m.clip_t0 }], { focus: "clip", single: true }) : mediaHtml(c, true);
  return player + notebarHtml(m.clip ? "note at 0.00 s" : "note on the frame") + notesListHtml(c, d) + `
  <footer class="dc-foot">
    <button class="btn primary" data-send ${d.notes.length ? "" : "disabled"}>${d.notes.length ? `Send ${d.notes.length} note${d.notes.length === 1 ? "" : "s"}` : "Add notes, then send"}</button>
    <button class="btn" data-none>No notes, looks right</button>
    <span class="dc-keys">${m.clip ? keyHint("Space", "play") + keyHint("I O", "range") : ""}${keyHint("N", "note")}</span>
  </footer>`;
}

/* approve */
function approveHtml(c, d) {
  const what = { brief: "the brief", direction: "the chosen direction", dramaturgy: "the dramaturgy", storyboard: "the storyboard", music: "the music", variant: "the hero variant", render: "the render", lock: "a lock", item: "this item" }[c.target] || c.target;
  return `<div class="approve-line"><span class="label">Approve</span><span>${esc(what)}${c.rev != null ? ` <span class="mono">rev ${c.rev}</span>` : ""}${c.item ? ` <span class="mono">${esc(c.item)}</span>` : ""}</span>
    ${["brief", "direction", "dramaturgy", "storyboard", "variant", "render"].includes(c.target) ? `<a class="btn ghost small" href="${projectPath(slug, { direction: "directions", variant: "variants" }[c.target] || c.target)}" data-link>Open it</a>` : ""}</div>
  <div class="changes" ${d.changes ? "" : "hidden"}><textarea data-note data-ctext rows="3" placeholder="What should change before you approve?">${esc(d.text)}</textarea></div>
  <footer class="dc-foot">
    ${d.changes ? `<button class="btn primary" data-reject>Send changes</button><button class="btn ghost" data-changes>Cancel</button>`
      : `<button class="btn primary" data-accept>Approve${c.rev != null ? " rev " + c.rev : ""}</button><button class="btn" data-changes>Request changes</button>`}
    <span class="dc-keys">${keyHint("Enter", "approve")}${keyHint("N", "request changes")}</span>
  </footer>`;
}

/* question */
function questionHtml(c, d) {
  return `<div class="proposed"><div class="label">Proposed</div><p>${esc(c.default)}</p></div>
  <textarea class="dc-text" data-note data-ctext rows="2" placeholder="${esc(c.placeholder || "Or write your own answer")}">${esc(d.text)}</textarea>
  <footer class="dc-foot">
    <button class="btn primary" data-accept>Accept proposed</button>
    <button class="btn" data-send ${d.text.trim() ? "" : "disabled"}>Send my answer</button>
    <span class="dc-keys">${keyHint("Enter", "accept")}${keyHint("N", "write")}${keyHint("Ctrl Enter", "send text")}</span>
  </footer>`;
}

function rerenderCard(c) { delete cardKeys[c.id]; renderStream(); setActive(c.id); }
function wireCard(n, c) {
  const d = draft(c.id);
  n.addEventListener("pointerdown", () => { if (activeCard !== c.id) setActive(c.id); });
  n.addEventListener("focusin", () => { if (activeCard !== c.id) setActive(c.id); });
  $$("[data-ctext]", n).forEach(i => i.addEventListener("input", () => {
    d.text = i.value;
    if (c.type === "question") { const b = $("[data-send]", n); if (b) b.disabled = !d.text.trim(); }
  }));
  $$("[data-ctext]", n).forEach(i => i.addEventListener("keydown", e => {
    if (e.key === "Enter" && (c.type !== "question" && c.type !== "approve" || e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      if (c.type === "approve") return reject(c);
      if (c.type === "question") return d.text.trim() && answer(c, { text: d.text.trim() });
      submit(c);
    }
  }));
  $$("video[data-hover]", n).forEach(v => {
    const b = v.closest(".copt");
    b.addEventListener("mouseenter", () => { if (!reduceMotion.matches) { v.classList.add("on"); v.play().catch(() => {}); } });
    b.addEventListener("mouseleave", () => { v.classList.remove("on"); v.pause(); });
  });
  if (c.type === "choice") {
    $$("[data-opt]", n).forEach(b => b.onclick = e => { if (!e.target.closest("audio")) selectOpt(c, b.dataset.opt); });
    $$("[data-oaudio]", n).forEach(a => a.addEventListener("play", () => $$("audio").forEach(x => { if (x !== a) x.pause(); })));
    $$("[data-mixattr]", n).forEach(b => b.onclick = () => {
      d.mix[b.dataset.mixattr] = b.dataset.from; d.mixOn = true;
      $$(`[data-mixattr="${CSS.escape(b.dataset.mixattr)}"]`, n).forEach(x => x.classList.toggle("on", x === b));
      $("[data-send]", n).textContent = "Send mix";
    });
    const mb = $(".mixbox", n); if (mb) mb.addEventListener("toggle", () => { d.mixOn = mb.open; $("[data-send]", n).textContent = d.mixOn ? "Send mix" : d.sel ? `Send: ${optLabel(c, d.sel)}` : "Select an option"; });
  }
  if (c.type === "compare" || (c.type === "note" && c.media?.clip)) {
    const p = players[c.id] = new Sync($(".sync", n), c.type === "compare" ? compareClips(c) : [{ id: "clip", src: media(c.media.clip), cue: c.media.cue, dur: c.media.dur, clip_t0: c.media.clip_t0 }], {
      focus: d.sel || c.default || (c.options ? c.options[0].id : "clip"),
      on: {
        time: at => { const l = $("[data-noteat]", n); if (l) l.textContent = `note${c.type === "compare" ? " on " + optLabel(c, at.clip) : ""} ${at.t0 != null ? rangeTxt(at.t0, at.t1) : "at " + secs(at.t) + " s"}`; },
        range: r => setCtx({ range: r ? { ...r, src: c.type === "compare" ? (c.options.find(o => o.id === r.clip) || {}).clip : c.media.clip, label: c.type === "compare" ? optLabel(c, r.clip) : c.title } : null }),
      },
    });
    p.restore(d.ps); d.ps = null;
    if (c.type === "compare") $$("[data-opt]", n).forEach(b => b.onclick = () => selectOpt(c, b.dataset.opt));
    $$("[data-branch]", n).forEach(b => b.onclick = async () => {
      const text = await ask(`Branch ${optLabel(c, b.dataset.branch)}`, "A child of this clip. Change only one thing; everything else stays.", "e.g. land the signet half a beat later");
      if (text) answer(c, { branch: b.dataset.branch, text, notes: d.notes });
    });
    p.readout();
  }
  if (c.type === "compare" || c.type === "note") {
    const inp = $("[data-nt]", n);
    const add = () => {
      const text = inp.value.trim(); if (!text) return toast("Write the note first");
      const p = players[c.id];
      const at = p ? p.at() : {};
      if (c.type === "note") delete at.clip;
      const note = { ...at, text };
      const tf = at.t_film ?? at.t0_film ?? (c.media?.clock === "film" ? at.t ?? at.t0 : null);
      note.beat = c.beat || ctx.beat || (tf != null ? beatAtFilm(tf) : null) || undefined;
      if (!note.beat) delete note.beat;
      d.notes.push(note); inp.value = "";
      rerenderCard(c);
      $(`.dcard[data-card="${CSS.escape(c.id)}"] [data-nt]`)?.focus();
    };
    $("[data-addnote]", n).onclick = add;
    inp.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); add(); } });
    $$("[data-rmnote]", n).forEach(b => b.onclick = () => { d.notes.splice(+b.dataset.rmnote, 1); rerenderCard(c); });
  }
  const sendB = $("[data-send]", n); if (sendB) sendB.onclick = () => submit(c);
  const acc = $("[data-accept]", n); if (acc) acc.onclick = () => accept(c);
  const none = $("[data-none]", n); if (none) none.onclick = async () => {
    if (c.type === "note") return answer(c, { none: true, text: d.text || undefined });
    const text = await ask("None of these", "What should a new attempt do differently?", "e.g. the list folds away like paper");
    if (text) answer(c, { none: true, text, notes: d.notes });
  };
  $$("[data-changes]", n).forEach(b => b.onclick = () => { d.changes = !d.changes; rerenderCard(c); if (d.changes) $(`.dcard[data-card="${CSS.escape(c.id)}"] textarea`)?.focus(); });
  const rej = $("[data-reject]", n); if (rej) rej.onclick = () => reject(c);
}
function selectOpt(c, id) {
  const d = draft(c.id);
  const o = c.options.find(x => x.id === id); if (!o || o.status === "planned") return;
  d.sel = d.sel === id && c.type === "choice" ? null : id; d.mixOn = false;
  rerenderCard(c);
  if (c.type === "compare" && players[c.id]) { players[c.id].setFocus(id); draft(c.id).ps = null; }
}
function submit(c) {
  const d = draft(c.id);
  if (c.type === "choice") {
    if (d.mixOn && c.attributes?.length) return answer(c, { mix: c.attributes.map(a => ({ attribute: a, from: d.mix[a] || c.default || c.options[0].id })), text: d.text.trim() || undefined });
    if (!d.sel) return c.default ? accept(c) : toast("Select an option first");
    return answer(c, { pick: d.sel, text: d.text.trim() || undefined });
  }
  if (c.type === "compare") {
    if (!d.sel && !d.notes.length) return c.default ? accept(c) : toast("Select a clip or add a note");
    return answer(c, { pick: d.sel || undefined, notes: d.notes, text: d.text.trim() || undefined });
  }
  if (c.type === "note") return d.notes.length ? answer(c, { notes: d.notes }) : toast("Add a note first, or send No notes");
  if (c.type === "question") return d.text.trim() ? answer(c, { text: d.text.trim() }) : accept(c);
  if (c.type === "approve") return d.changes ? reject(c) : accept(c);
}
function accept(c) {
  const d = draft(c.id);
  if (c.type === "approve") return answer(c, { approve: true });
  if (c.type === "compare") return answer(c, { accept: true, notes: d.notes, text: d.text.trim() || undefined });
  if (c.type === "choice" && !c.default) return toast("No proposed default on this card");
  return answer(c, { accept: true, text: c.type === "choice" ? d.text.trim() || undefined : undefined });
}
function reject(c) { const d = draft(c.id); if (!d.text.trim()) return toast("Say what should change"); answer(c, { approve: false, text: d.text.trim() }); }
async function answer(c, payload) {
  const body = { v: c.v, ...payload };
  if (body.notes && !body.notes.length) delete body.notes;
  const cx = ctxPayload(); if (cx) body.context = cx;
  try {
    const j = await api(`/api/p/${encodeURIComponent(slug)}/cards/${encodeURIComponent(c.id)}/answer`, body);
    toast(`Sent: ${c.title} (#${j.seq}). The agent sees it in its inbox.`);
    delete drafts[c.id]; clearCtx();
    const open = openCards(), i = open.findIndex(x => x.id === c.id);
    activeCard = open[i + 1]?.id || open[i - 1]?.id || null;
  } catch (e) { toast("Not sent: " + e.message); }
  if (await loadProject(false)) draw(false);
}

/* history: answered and withdrawn cards, collapsed */
function answerSummary(c) {
  const a = c.answer || {};
  if (c.status === "withdrawn") return "withdrawn by the agent";
  const nn = a.notes?.length ? `${a.notes.length} note${a.notes.length === 1 ? "" : "s"}` : "";
  let s = "";
  if (c.type === "choice") s = a.mix ? "mix: " + a.mix.map(m => `${m.attribute} from ${optLabel(c, m.from)}`).join(", ") : optLabel(c, a.pick) + (a.accepted_default ? " (proposed)" : "");
  else if (c.type === "compare") s = [a.pick ? optLabel(c, a.pick) + (a.accepted_default ? " (proposed)" : "") : a.branch ? "branch " + optLabel(c, a.branch) : a.none ? "none of these" : "", nn].filter(Boolean).join(" + ");
  else if (c.type === "note") s = nn || (a.none ? "no notes" : "");
  else if (c.type === "approve") s = a.approve ? `approved${a.rev != null ? " rev " + a.rev : ""}` : "changes requested";
  else if (c.type === "question") s = a.accepted_default ? "accepted: " + a.text : a.text;
  if (a.text && c.type !== "question") s += `: "${a.text}"`;
  return s;
}
function renderHistory() {
  const done = cardsAll().filter(c => c.status !== "open").sort((a, b) => Date.parse(b.updated) - Date.parse(a.updated));
  const el = $("#history");
  if (!done.length) { el.innerHTML = ""; return; }
  const key = JSON.stringify(done.map(c => [c.id, c.v])) + D.feedback_cursor;
  if (el.dataset.key === key) return;
  el.dataset.key = key;
  const wasOpen = $("details.hist", el)?.open;
  el.innerHTML = `<details class="hist" ${wasOpen ? "open" : ""}><summary><span class="label">History</span><span class="muted">${done.length} answered or withdrawn</span></summary>
    <div class="hlist">${done.map(c => {
      const a = c.answer || {}, read = a.seq != null && a.seq <= D.feedback_cursor;
      return `<details class="hcard"><summary><span class="dc-type t-${esc(c.type)}"><i aria-hidden="true"></i>${CARD_TYPE_LABEL[c.type]}</span><span class="ht">${esc(c.title)}</span><span class="ha">${esc(answerSummary(c))}</span>
        <span class="chip ${c.status === "withdrawn" ? "" : read ? "good" : "accent"}">${c.status === "withdrawn" ? "withdrawn" : read ? "read by agent" : "not read yet"}</span><span class="mono muted">${esc(ago(c.updated))}</span></summary>
        <div class="hbody">${bodyHtml(c.body)}
          ${(c.options || []).length ? `<ul class="hopts">${c.options.map(o => `<li class="${a.pick === o.id || a.branch === o.id ? "on" : ""}">${o.still ? `<img src="${media(o.still)}" alt="" loading="lazy">` : ""}<b>${esc(o.label)}</b> <span class="muted">${esc(o.detail || "")}</span></li>`).join("")}</ul>` : ""}
          ${a.notes?.length ? `<ul class="dnotes">${a.notes.map(n => `<li><span class="mono">${n.clip && c.type === "compare" ? esc(optLabel(c, n.clip)) + " " : ""}${n.t0 != null ? rangeTxt(n.t0, n.t1) : n.t != null ? "@ " + secs(n.t) + " s" : ""}${n.beat ? " · " + esc(n.beat) : ""}</span><span>${esc(n.text)}</span><span></span></li>`).join("")}</ul>` : ""}
          ${a.context ? `<div class="muted small">context: ${esc(JSON.stringify(a.context))}</div>` : ""}
          <div class="mono muted">${esc(c.id)} · v${c.v}${a.seq ? " · feedback #" + a.seq : ""}</div></div></details>`;
    }).join("")}</div></details>`;
}

/* the aside: agent line, brief at a glance, locks, sent */
function renderSide() {
  const c = D.card, a = c.agent || {}, b = S.brief || {};
  const st = a.state || "idle";
  const key = JSON.stringify([a, b.audience, b.outcome, b.priority, b.envelope, S.locks, D.feedback_unread, c.stage, c.pipeline]);
  const el = $("#side");
  if (el.dataset.key === key) return;
  el.dataset.key = key;
  const locks = S.locks || [];
  const line = (k, v, f) => v ? `<div class="glance-row"><span class="label">${k}</span><span>${esc(v)}</span></div>` : "";
  el.innerHTML = `
    <div class="card pad side-agent"><div class="label">Agent</div>
      <div class="agent-line"><span class="adot ${esc(st)}"></span><b>${esc({ working: "Working", waiting: "Waiting for you", idle: "Idle" }[st] || st)}</b><span class="muted">${esc(ago(a.at))}</span></div>
      ${a.note ? `<p>${esc(a.note)}</p>` : ""}
      <div class="muted small">${D.feedback_unread ? `${D.feedback_unread} sent, not read yet` : "Everything you sent has been read."} <button class="linkbtn" data-open-outbox>Outbox</button></div></div>
    <div class="card pad side-brief"><div class="glance-head"><span class="label">Brief</span><a class="btn ghost small" href="${projectPath(slug, "brief")}" data-link>Open</a></div>
      ${line("Audience", b.audience?.value)}${line("Outcome", b.outcome?.value)}${line("Must land", b.priority?.value)}
      ${line("Delivery", c.delivery)}
      ${!b.audience?.value && !b.outcome?.value ? `<p class="muted" style="margin:6px 0 0">Not discussed yet.</p>` : ""}</div>
    ${locks.length ? `<div class="card pad side-locks"><div class="label" style="margin-bottom:8px">Locks</div><div class="chips">${locks.map(l => `<span class="chip ${l.kind}" title="${esc(l.kind)} · ${esc(l.scope)}"><b>${l.kind}</b>${esc(l.text)}</span>`).join("")}</div></div>` : ""}
    <p class="side-tip muted small">Select a beat, a point in a still or a range in a player and it goes with your next answer as context.</p>`;
  $("[data-open-outbox]", el).onclick = openOutbox;
}

/* ---------- routing ---------- */
function parseRoute() {
  const m = location.pathname.match(/^\/p\/([^/]+)\/?([^/]*)\/?$/);
  if (m) return { view: "project", slug: decodeURIComponent(m[1]), stage: STAGE_IDS.includes(m[2]) ? m[2] : null };
  return { view: "home", q: new URLSearchParams(location.search).get("q") || "" };
}
function go(path, { replace = false } = {}) {
  if (path === location.pathname + location.search) return;
  history[replace ? "replaceState" : "pushState"](null, "", path);
  onRoute();
}
const projectPath = (sl, st) => `/p/${encodeURIComponent(sl)}/${st || ""}`;
window.addEventListener("popstate", onRoute);
document.addEventListener("click", e => {
  const a = e.target.closest("a[data-link]");
  if (!a || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
  e.preventDefault(); closeOverlays(); go(a.getAttribute("href"));
});

function showView(id) {
  for (const v of ["home", "project", "missing"]) {
    const el = $("#" + v), on = v === id;
    if (on && el.hidden) { el.hidden = false; el.classList.remove("enter"); void el.offsetWidth; el.classList.add("enter"); }
    else if (!on) el.hidden = true;
  }
  $("#projhead").classList.toggle("hidden", id !== "project");
  $("#open-outbox").classList.toggle("hidden", id !== "project");
  if (id !== "project") { showPanel(false); renderCtx(); }
}

async function onRoute() {
  route = parseRoute();
  if (route.view !== "project") stopPlayers();
  if (route.view === "home") {
    if (route.q !== undefined) home.q = route.q;
    if (!P.length) await loadProjects();
    showView("home"); renderCrumbs(); renderHome();
    document.title = "Projects | Director's desk";
    return;
  }
  if (route.slug !== slug) {
    stopPlayers();
    slug = route.slug; S = D = null; version = mediaV = ""; keys = {}; ui = freshUi(); selected = null;
    $$("section.stage").forEach(s => s.innerHTML = "");
    $("#stream").innerHTML = ""; $("#history").innerHTML = ""; $("#history").dataset.key = ""; $("#side").dataset.key = "";
    for (const k in cardKeys) delete cardKeys[k];
    for (const k in players) delete players[k];
    activeCard = null; clearCtx();
    const ok = await loadProject(true);
    if (!ok) return;
    window.scrollTo(0, 0);
  }
  if (selected !== route.stage && selected === "variants") stopPlayers();
  selected = route.stage;
  showView("project"); draw(false); showPanel(!!selected);
}

/* ---------- data ---------- */
async function loadProjects() {
  try {
    const j = await api("/api/projects");
    P = j.projects; homeV = j.version; single = j.single;
    setLive(true);
    return true;
  } catch (e) { setLive(false); return false; }
}
async function loadProject(first) {
  try {
    const d = await api(`/api/p/${encodeURIComponent(slug)}/state`);
    D = d; S = d.state; version = d.version; mediaV = d.media_version;
    const i = P.findIndex(p => p.slug === slug);
    if (i >= 0) P[i] = d.card; else P.push(d.card);
    setLive(true);
    return true;
  } catch (e) {
    if (first) {
      showView("missing"); renderCrumbs();
      $("#missing").innerHTML = `<div class="empty"><div class="empty-art"></div><div class="t">No project called "${esc(slug)}"</div>
        <p>It may have been renamed, or the desk serves another root.</p><div class="acts"><a class="btn primary" href="/" data-link>All projects</a></div></div>`;
      document.title = "Not found | Director's desk";
    } else setLive(false);
    return false;
  }
}
async function poll() {
  try {
    const v = await api(`/api/version${slug && route.view === "project" ? "?p=" + encodeURIComponent(slug) : ""}`);
    setLive(true);
    if (v.home !== homeV) {
      await loadProjects();
      if (route.view === "home") renderHome();
      if (!$("#switcher").hidden) renderSwitcher();
    }
    if (route.view === "project" && v.project && v.project !== version) { if (await loadProject(false)) draw(false); }
  } catch (e) { setLive(false); }
}

async function send(action, payload = {}, stage) {
  const body = { action, stage: stage || selected || S.stage, ...payload };
  const cx = ctxPayload(); if (cx && !body.context) body.context = cx;
  try {
    const j = await api(`/api/p/${encodeURIComponent(slug)}/feedback`, body);
    toast(`Sent to the agent: ${action} (#${j.seq})`);
    if (cx) clearCtx();
    if (await loadProject(false)) draw(false);
    return j;
  } catch (e) { toast("Not sent: " + e.message); }
}

/* ---------- header: crumbs, project head, pipeline ---------- */
function current() { return P.find(p => p.slug === slug) || (D && D.card); }
function renderCrumbs() {
  const c = route.view === "project" ? current() : null;
  const chevron = `<svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M3 4.5 6 7.5 9 4.5"/></svg>`;
  let h = `<a class="crumb-root ${route.view === "home" ? "crumb here" : ""}" href="/" data-link>Projects</a>`;
  if (route.view === "project" && c) {
    if (c.client) h += `<span class="sep opt" aria-hidden="true">/</span><a class="crumb-client" href="/?q=${encodeURIComponent(c.client)}" data-link title="All projects for ${esc(c.client)}">${esc(c.client)}</a>`;
    h += `<span class="sep" aria-hidden="true">/</span><button class="switch-btn" id="switch-btn" aria-haspopup="dialog" aria-expanded="false" title="Switch project">
      ${c.poster ? `<img class="thumb" src="${fileUrl(c.slug, c.poster)}" alt="">` : `<span class="thumb"></span>`}<span class="nm">${esc(c.name)}</span>${chevron}</button>`;
    if (selected) h += `<span class="sep opt" aria-hidden="true">/</span><span class="crumb crumb-stage here" aria-current="page">${esc(stageName(selected))}</span>`;
  }
  $("#crumbs").innerHTML = h;
  const sb = $("#switch-btn"); if (sb) sb.onclick = () => toggleSwitcher();
}
function renderProjectHead() {
  const c = D.card;
  $("#ptitle").textContent = c.name;
  const bits = [
    c.client ? `<span class="bit">${esc(c.client)}</span>` : "",
    c.delivery ? `<span class="bit opt">${esc(c.delivery)}</span>` : "",
    `<span class="bit opt mono">rev ${esc(S.revision ?? 0)}</span>`,
    `<span class="bit"><span class="status-dot ${esc(c.status)}">${esc(c.status)}</span></span>`,
  ].filter(Boolean);
  $("#pmeta").innerHTML = `${badgeHtml(c, true)}` + bits.join("");
  const archived = c.status === "archived";
  $("#pactions").innerHTML = single ? "" : `<button class="btn ghost small" id="parchive" title="${archived ? "Back to the active projects" : "Hide from the active projects; nothing is deleted"}">
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="2" y="3" width="12" height="3.2" rx="1"/><path d="M3.2 6.2V12a1 1 0 0 0 1 1h7.6a1 1 0 0 0 1-1V6.2M6.5 9h3"/></svg><span class="lbl">${archived ? "Unarchive" : "Archive"}</span></button>`;
  const pa = $("#parchive"); if (pa) pa.onclick = () => setArchived(c.slug, !archived);
  // state rail: one compact item per stage; it opens the stage as a panel
  const open = (D.cards || []).filter(x => x.status === "open");
  const railHtml = `<a class="rstep rs-stream ${selected ? "" : "on"}" href="${projectPath(slug)}" data-link ${selected ? "" : 'aria-current="page"'}>
      <span class="rdot">${open.length || ""}</span><span class="rname">Stream</span></a><span class="rsep" aria-hidden="true"></span>` +
    RAIL.map(s => {
      const st = c.pipeline[s.n] || "todo";
      const n = open.filter(x => x.stage === s.id).length;
      const inner = st === "done" ? `<svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="M2.5 6.3 5 8.7 9.5 3.5"/></svg>` : st === "stale" ? "!" : "";
      return `<a class="rstep ${st} ${s.id === selected ? "on" : ""}" href="${projectPath(slug, s.id)}" data-link data-rail="${s.id}" ${s.id === selected ? 'aria-current="page"' : ""}
        title="${s.name}${PSTATE_LABEL[st] ? ": " + PSTATE_LABEL[st] : ""}${n ? `, ${n} open card${n === 1 ? "" : "s"}` : ""} (Shift+${s.n})"><span class="rdot">${inner}</span><span class="rname">${s.name}</span>${n ? `<span class="rcount">${n}</span>` : ""}</a>`;
    }).join("");
  const rail = $("#rail");
  if (rail.innerHTML !== railHtml) rail.innerHTML = railHtml;
  const tab = $(".rstep.on", rail);
  if (tab && rail.scrollWidth > rail.clientWidth) rail.scrollLeft = Math.max(0, tab.offsetLeft - (rail.clientWidth - tab.offsetWidth) / 2);
  // outbox button
  const ob = $("#open-outbox");
  ob.classList.toggle("has", D.feedback_unread > 0);
  $("#outbox-count").textContent = D.feedback_unread;
  ob.title = `${D.feedback_unread} waiting for the agent, ${D.feedback_total} sent in total (O)`;
  document.title = `${selected ? stageName(selected) : "Stream"} | ${c.name}`;
}
function selectStage(id) { if (STAGE_IDS.includes(id)) go(projectPath(slug, id)); }
let panelReturn = null;
function showPanel(on) {
  const p = $("#panel"), was = !p.hidden;
  if (on) {
    $("#panel-title").textContent = `${stageName(selected)}`;
    const i = STAGE_IDS.indexOf(selected);
    $("#panel-prev").disabled = i <= 0; $("#panel-next").disabled = i >= STAGE_IDS.length - 1;
    if (!was) {
      panelReturn = document.activeElement;
      p.hidden = false; document.body.classList.add("noscroll");
      $(".panel-head [data-close-panel]").focus({ preventScroll: true });
    }
    $("#panel-body").scrollTop = was ? $("#panel-body").scrollTop : 0;
  } else if (was) {
    p.hidden = true; document.body.classList.remove("noscroll");
    if (vplayer) vplayer.stop();
    if (panelReturn && document.contains(panelReturn)) panelReturn.focus({ preventScroll: true });
  }
}
function closePanel() { if (route.view === "project" && selected) go(projectPath(slug)); }
$$("[data-close-panel]").forEach(b => b.onclick = closePanel);
$("#panel-prev").onclick = () => { const i = STAGE_IDS.indexOf(selected); if (i > 0) go(projectPath(slug, STAGE_IDS[i - 1]), { replace: true }); };
$("#panel-next").onclick = () => { const i = STAGE_IDS.indexOf(selected); if (i < STAGE_IDS.length - 1) go(projectPath(slug, STAGE_IDS[i + 1]), { replace: true }); };

/* ---------- outbox drawer ---------- */
function summary(f) {
  if (f.action === "note") return `${f.variant || f.beat || ""}${f.t != null ? " @ " + fmt(f.t) : ""}: ${f.text || ""}`;
  if (f.action === "pick") { const t = f.target || {}; return t.type === "kind" ? `${t.beat}: ${t.kind}` : `${t.type || ""} ${t.id || ""}`; }
  if (f.action === "approve") return `${f.target} rev ${f.rev}`;
  if (f.action === "reorder") return (f.order || []).join(" ");
  if (f.action === "mix") return f.text || JSON.stringify(f.attributes || f.variants || {});
  if (f.action === "new-project") return (f.spec && f.spec.name) || f.text || "";
  return f.text || "";
}
function renderOutbox() {
  if (!D) return;
  const fb = (D.feedback || []).slice().reverse();
  $("#outbox-sub").textContent = D.feedback_unread ? `${D.feedback_unread} waiting for the agent. It reads them, acts, and marks them read.` : "Nothing waiting. Every choice you make here lands in this project's feedback.jsonl.";
  $("#outbox-list").innerHTML = fb.length ? fb.map(f => {
    const read = f.seq <= D.feedback_cursor;
    return `<div class="ob-item"><span class="mono muted">#${f.seq}</span><div class="what"><b>${esc(f.action)}</b> ${esc(summary(f))}<div class="when">${esc(stageName(f.stage || ""))} · ${esc(ago(f.ts))}</div></div><span class="chip ${read ? "good" : "accent"}">${read ? "read" : "waiting"}</span></div>`;
  }).join("") : `<div class="ob-item"><span></span><span class="muted">Nothing sent yet.</span><span></span></div>`;
}
function openOutbox() { if (route.view !== "project") return; closeOverlays(); renderOutbox(); $("#outbox").hidden = false; $(".drawer-panel [data-close-outbox]").focus(); }
$("#open-outbox").onclick = openOutbox;
$$("[data-close-outbox]").forEach(b => b.onclick = () => { $("#outbox").hidden = true; $("#open-outbox").focus(); });

/* ---------- switcher ---------- */
function toggleSwitcher(force) {
  const sw = $("#switcher"), open = force ?? sw.hidden;
  const btn = $("#switch-btn");
  if (!open) { sw.hidden = true; if (btn) { btn.setAttribute("aria-expanded", "false"); btn.focus(); } return; }
  closeOverlays();
  renderSwitcher();
  sw.hidden = false;
  if (btn) {
    btn.setAttribute("aria-expanded", "true");
    const r = btn.getBoundingClientRect(), panel = $(".switcher-panel");
    panel.style.left = Math.max(12, Math.min(r.left, innerWidth - 392)) + "px";
    panel.style.top = r.bottom + 8 + "px";
  }
  ($(".sw-item[aria-current]") || $(".sw-item"))?.focus();
}
function renderSwitcher() {
  const list = sortProjects(P.filter(p => p.status !== "archived" || p.slug === slug), "updated");
  $("#sw-list").innerHTML = list.map(p => `<a class="sw-item" href="${projectPath(p.slug)}" data-link ${p.slug === slug ? 'aria-current="page"' : ""}>
      ${p.poster ? `<img src="${fileUrl(p.slug, p.poster)}" alt="">` : `<span class="ph"></span>`}
      <span style="min-width:0"><div class="nm">${esc(p.name)}</div><div class="cl">${esc(p.client || "No client")}</div></span>
      <span class="st"><i class="${esc(p.badge || "")}"></i>${p.stage_index}/6</span></a>`).join("") || `<div class="pal-empty">No projects yet.</div>`;
  $("[data-new-project]", $("#switcher")).hidden = single;
}
$$("[data-close-switcher]").forEach(b => b.onclick = () => toggleSwitcher(false));
$("#switcher").addEventListener("keydown", e => {
  const items = $$(".sw-item, .sw-link", $("#switcher")).filter(x => !x.hidden), i = items.indexOf(document.activeElement);
  if (e.key === "ArrowDown") { e.preventDefault(); items[(i + 1) % items.length]?.focus(); }
  if (e.key === "ArrowUp") { e.preventDefault(); items[(i - 1 + items.length) % items.length]?.focus(); }
});

/* ---------- home ---------- */
function sortProjects(list, by) {
  const l = list.slice();
  if (by === "name") l.sort((a, b) => a.name.localeCompare(b.name));
  else if (by === "stage") l.sort((a, b) => b.stage_index - a.stage_index || Date.parse(b.updated) - Date.parse(a.updated));
  else l.sort((a, b) => Date.parse(b.updated) - Date.parse(a.updated));
  return l;
}
function filtered() {
  const q = home.q.trim().toLowerCase();
  return sortProjects(P.filter(p => (home.filter === "all" ? true : p.status === home.filter) &&
    (!q || [p.name, p.client, p.goal, p.slug, ...(p.tags || [])].join(" ").toLowerCase().includes(q))), home.sort);
}
function renderHome() {
  const el = $("#home");
  const counts = Object.fromEntries(STATUS_ORDER.map(s => [s, P.filter(p => p.status === s).length]));
  const waiting = P.filter(p => p.status === "active" && p.badge === "waiting").length;
  const pend = P.reduce((n, p) => n + (p.status !== "archived" ? p.pending : 0), 0);
  const ncards = P.reduce((n, p) => n + (p.status !== "archived" ? p.open_cards || 0 : 0), 0);
  const list = filtered();
  const filters = [["active", "Active"], ["paused", "Paused"], ["done", "Done"], ["archived", "Archived"], ["all", "All"]];
  const subBits = [`<b>${counts.active}</b> active`];
  if (ncards) subBits.push(`<b>${ncards}</b> open card${ncards === 1 ? "" : "s"} for you`);
  if (waiting) subBits.push(`<b>${waiting}</b> waiting for you`);
  if (pend) subBits.push(`<b>${pend}</b> action${pend === 1 ? "" : "s"} with the agent`);
  el.innerHTML = `
    <div class="home-head">
      <div><h1>Projects</h1><div class="sub">${subBits.join(" · ")}</div></div>
      <div class="actions">${single ? "" : `<button class="btn primary" data-new-project><svg width="13" height="13" viewBox="0 0 14 14" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M7 2v10M2 7h10"/></svg>New project</button>`}</div>
    </div>
    ${inboxHtml()}
    <div class="toolbar">
      <div class="seg" role="group" aria-label="Filter by status">${filters.map(([k, l]) => `<button aria-pressed="${home.filter === k}" data-filter="${k}">${l}<span class="n">${k === "all" ? P.length : counts[k]}</span></button>`).join("")}</div>
      <label class="search"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5 14 14"/></svg>
        <input type="search" id="hq" placeholder="Search name, client, tag" value="${esc(home.q)}" aria-label="Search projects"><kbd>/</kbd></label>
      <label class="sort">Sort <select id="hsort" aria-label="Sort projects">
        <option value="updated" ${home.sort === "updated" ? "selected" : ""}>Recently updated</option>
        <option value="name" ${home.sort === "name" ? "selected" : ""}>Name</option>
        <option value="stage" ${home.sort === "stage" ? "selected" : ""}>Furthest stage</option></select></label>
    </div>
    <div id="hgrid">${gridHtml(list)}</div>`;
  $$("[data-filter]", el).forEach(b => b.onclick = () => { home.filter = b.dataset.filter; localStorage.setItem("desk-filter", home.filter); renderHome(); });
  const q = $("#hq", el);
  q.oninput = () => {
    home.q = q.value;
    history.replaceState(null, "", home.q ? `/?q=${encodeURIComponent(home.q)}` : "/");
    $("#hgrid").innerHTML = gridHtml(filtered()); wireGrid();
  };
  $("#hsort", el).onchange = e => { home.sort = e.target.value; localStorage.setItem("desk-sort", home.sort); $("#hgrid").innerHTML = gridHtml(filtered()); wireGrid(); };
  wireGrid();
}
function inboxHtml() {
  const items = P.filter(p => p.pending && p.status !== "archived");
  if (!items.length) return "";
  return `<section class="inbox" aria-label="Sent to the agent, not read yet">
    <div class="inbox-head"><div class="label">With the agent</div><span class="muted" style="font-size:12.5px">sent from the desk, not read yet</span></div>
    <div class="inbox-list">${items.map(p => `<a class="inbox-item" href="${projectPath(p.slug)}" data-link><span class="pj">${esc(p.name)}</span>
      <span class="what"><b>${p.pending} sent, unread by agent</b> · ${esc(stageName(p.stage))}${p.agent && p.agent.note ? " · " + esc(p.agent.note) : ""}</span><span class="mono muted">${esc(ago(p.updated))}</span></a>`).join("")}</div></section>`;
}
function gridHtml(list) {
  if (!P.length) return `<div class="empty"><div class="empty-art"></div><div class="t">No projects yet</div>
    <p>A project is one film: its brief, directions, beats and variants. Start one and the agent opens the brief dialogue.</p>
    <div class="acts">${single ? "" : `<button class="btn primary" data-new-project>New project</button>`}</div></div>`;
  if (!list.length) return `<div class="empty"><div class="empty-art"></div><div class="t">Nothing here</div>
    <p>No ${home.filter === "all" ? "" : esc(home.filter) + " "}projects${home.q ? ` match "${esc(home.q)}"` : ""}.</p>
    <div class="acts">${home.q ? `<button class="btn" data-clear-q>Clear search</button>` : ""}${home.filter !== "all" ? `<button class="btn" data-filter-all>Show all</button>` : ""}</div></div>`;
  const cards = list.map(p => {
    const cur = STAGES[p.stage_index] || STAGES[0];
    return `<a class="pcard ${esc(p.status)}" href="${projectPath(p.slug)}" data-link data-slug="${esc(p.slug)}" ${p.preview ? `data-preview="${esc(fileUrl(p.slug, p.preview))}"` : ""}>
      <div class="poster">
        ${p.poster ? `<img src="${fileUrl(p.slug, p.poster)}" alt="" loading="lazy">` : `<div class="noposter"><span>${esc(p.name.slice(0, 1))}</span></div>`}
        <span class="pbadge">${badgeHtml(p)}</span>
        ${p.preview ? `<span class="pplay" aria-hidden="true">preview</span>` : ""}
        <div class="pmini" aria-label="Stage ${cur.n} of 6, ${esc(cur.name)}">${p.pipeline.map((st, i) => `<i class="${st}" title="${STAGES[i].name}: ${PSTATE_LABEL[st] || "to do"}"></i>`).join("")}</div>
      </div>
      <div class="pbody">
        <h3>${esc(p.name)}</h3>
        <div class="client">${esc(p.client || "No client")}</div>
        ${p.goal ? `<p class="goal">${esc(p.goal)}</p>` : ""}
        <div class="pfoot">${p.open_cards ? `<span class="ncards">${p.open_cards} open card${p.open_cards === 1 ? "" : "s"}</span>` : ""}<span class="stg">Stage ${cur.n} · ${esc(cur.name)}</span><span class="status-dot ${esc(p.status)}">${esc(p.status)}</span><span class="upd">${esc(ago(p.updated))}</span></div>
      </div>
      ${p.status === "archived" && !single ? `<button class="btn small card-act" data-unarchive="${esc(p.slug)}">Unarchive</button>` : ""}
    </a>`;
  }).join("");
  const newCard = single || home.filter === "archived" ? "" : `<button class="newcard" data-new-project><span class="plus" aria-hidden="true">+</span><span class="t">New project</span><span class="s">Name, client, goal, aspect and length</span></button>`;
  return `<div class="pgrid">${cards}${newCard}</div>`;
}
function wireGrid() {
  const g = $("#hgrid");
  $$("[data-clear-q]", g).forEach(b => b.onclick = () => { home.q = ""; history.replaceState(null, "", "/"); renderHome(); });
  $$("[data-filter-all]", g).forEach(b => b.onclick = () => { home.filter = "all"; renderHome(); });
  $$("[data-unarchive]", g).forEach(b => b.onclick = e => { e.preventDefault(); e.stopPropagation(); setArchived(b.dataset.unarchive, false); });
  $$(".pcard[data-preview]", g).forEach(card => {
    const poster = $(".poster", card);
    let v = null;
    const start = () => {
      if (reduceMotion.matches) return;
      if (!v) { v = document.createElement("video"); v.muted = true; v.loop = true; v.playsInline = true; v.preload = "auto"; v.src = card.dataset.preview; v.setAttribute("aria-hidden", "true"); poster.insertBefore(v, $(".pbadge", poster)); }
      v.currentTime = 0;
      v.play().then(() => v.classList.add("on")).catch(() => {});
    };
    const stop = () => { if (v) { v.classList.remove("on"); v.pause(); } };
    card.addEventListener("mouseenter", start); card.addEventListener("mouseleave", stop);
    card.addEventListener("focus", start); card.addEventListener("blur", stop);
  });
}
document.addEventListener("click", e => { if (e.target.closest("[data-new-project]")) { e.preventDefault(); openNew(); } });

async function setArchived(sl, archived) {
  try {
    const j = await api(`/api/p/${encodeURIComponent(sl)}/archive`, { archived });
    toast(archived ? "Archived. Find it under Archived." : `Back to ${j.status}.`);
    await loadProjects();
    if (route.view === "project" && sl === slug) { if (await loadProject(false)) draw(true); }
    else if (route.view === "home") renderHome();
  } catch (e) { toast("Could not change it: " + e.message); }
}

/* ---------- new project dialog ---------- */
function openNew() {
  if (single) return toast("This desk serves one project. Start it on a projects root to create more.");
  closeOverlays();
  const f = $("#newform"); f.reset(); $("#new-err").textContent = "";
  $("#newdlg").showModal(); f.elements.name.focus();
}
$$("#len-presets [data-len]").forEach(b => b.onclick = () => { $("#newform").elements.length_s.value = b.dataset.len; });
$("#newform").addEventListener("submit", async e => {
  e.preventDefault();
  const f = e.target, err = $("#new-err");
  const name = f.elements.name.value.trim();
  const aspect = $$('input[name="aspect"]:checked', f).map(x => x.value);
  const length_s = +f.elements.length_s.value;
  if (!name) { err.textContent = "Give it a name."; f.elements.name.focus(); return; }
  if (!aspect.length) { err.textContent = "Pick at least one aspect."; return; }
  if (!(length_s >= 1 && length_s <= 600)) { err.textContent = "Length between 1 and 600 seconds."; f.elements.length_s.focus(); return; }
  $("#new-submit").disabled = true;
  try {
    const j = await api("/api/projects", { name, client: f.elements.client.value.trim(), goal: f.elements.goal.value.trim(), aspect, length_s });
    $("#newdlg").close();
    await loadProjects();
    toast(`Created "${name}". The agent sees it in the inbox.`);
    go(projectPath(j.slug));
  } catch (x) { err.textContent = x.message; }
  finally { $("#new-submit").disabled = false; }
});
document.addEventListener("click", e => { const b = e.target.closest("[data-close-dialog]"); if (b) b.closest("dialog").close(); });
$$("dialog.modal").forEach(d => d.addEventListener("click", e => { if (e.target === d) d.close(); }));

/* ask: a small in-page prompt instead of window.prompt */
function ask(title, sub, placeholder = "") {
  return new Promise(resolve => {
    const d = $("#askdlg"), t = $("#ask-text");
    $("#ask-title").textContent = title; $("#ask-sub").textContent = sub; t.value = ""; t.placeholder = placeholder;
    const done = v => { d.removeEventListener("close", onClose); resolve(v); };
    const onClose = () => done(d.returnValue === "ok" ? t.value.trim() || null : null);
    d.returnValue = ""; d.addEventListener("close", onClose);
    d.showModal(); t.focus();
  });
}
$("#askform").addEventListener("submit", e => { e.preventDefault(); $("#askdlg").close("ok"); });
$("#ask-text").addEventListener("keydown", e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); $("#askdlg").close("ok"); } });

/* ---------- command palette ---------- */
let palItems = [], palSel = 0;
function fuzzy(q, text) {
  if (!q) return 1;
  text = text.toLowerCase(); q = q.toLowerCase();
  const i = text.indexOf(q);
  if (i >= 0) return 100 - i;
  let ti = 0, score = 0;
  for (const ch of q) { const j = text.indexOf(ch, ti); if (j < 0) return 0; score += j === ti ? 3 : 1; ti = j + 1; }
  return score;
}
function palCandidates(q) {
  const out = [];
  const cur = route.view === "project" ? current() : null;
  if (cur) {
    out.push({ group: `${cur.name}`, icon: "≡", title: "Stream", small: cur.open_cards ? `${cur.open_cards} open` : "", right: selected ? "" : "here", key: `${cur.name} stream cards`, run: () => go(projectPath(cur.slug)) });
    STAGES.forEach(s => out.push({ group: `${cur.name}`, icon: s.n, title: s.name, small: PSTATE_LABEL[cur.pipeline[s.n]] || "", right: s.id === selected ? "here" : "", key: `${cur.name} ${cur.client} ${s.name} stage ${s.n}`, run: () => go(projectPath(cur.slug, s.id)) }));
  }
  sortProjects(P.filter(p => p.status !== "archived" || q), "updated").forEach(p => out.push({ group: "Projects", img: p.poster ? fileUrl(p.slug, p.poster) : null, icon: p.name[0], title: p.name, small: p.client, right: `${p.stage_index} · ${stageName(p.stage)}`, key: `${p.name} ${p.client} ${p.slug} ${(p.tags || []).join(" ")} ${p.status}`, run: () => go(projectPath(p.slug)) }));
  if (q) P.forEach(p => { if (p.slug !== slug) STAGES.forEach(s => out.push({ group: "Stages in other projects", icon: s.n, title: `${p.name} / ${s.name}`, small: p.client, right: "", key: `${p.name} ${p.client} ${s.name}`, run: () => go(projectPath(p.slug, s.id)) })); });
  const cmds = [
    { title: "All projects", icon: "⌂", right: "G H", run: () => go("/") },
    !single && { title: "New project", icon: "+", right: "C", run: openNew },
    cur && !single && { title: cur.status === "archived" ? "Unarchive this project" : "Archive this project", icon: "↓", run: () => setArchived(cur.slug, cur.status !== "archived") },
    cur && { title: "Open the outbox", icon: "✉", right: "Shift O", run: openOutbox },
    { title: `Switch to ${root.dataset.theme === "light" ? "dark" : "light"} theme`, icon: "◐", run: toggleTheme },
    { title: "Keyboard shortcuts", icon: "?", right: "?", run: () => $("#helpdlg").showModal() },
  ].filter(Boolean).map(c => ({ ...c, group: "Commands", key: c.title }));
  out.push(...cmds);
  if (!q) return out.filter(o => o.group !== "Stages in other projects");
  return out.map(o => ({ o, s: fuzzy(q, o.key + " " + o.title) })).filter(x => x.s > 0).sort((a, b) => b.s - a.s).map(x => x.o).slice(0, 40);
}
function renderPalette() {
  const q = $("#pal-q").value.trim();
  palItems = palCandidates(q);
  palSel = Math.min(palSel, Math.max(0, palItems.length - 1));
  let lastGroup = null, h = "";
  palItems.forEach((it, i) => {
    if (!q && it.group !== lastGroup) { h += `<div class="pal-group label">${esc(it.group)}</div>`; lastGroup = it.group; }
    h += `<div class="pal-item" role="option" id="pal-${i}" data-i="${i}" aria-selected="${i === palSel}">
      <span class="pal-ic">${it.img ? `<img src="${it.img}" alt="">` : esc(it.icon ?? "")}</span>
      <span class="pal-t">${esc(it.title)}${it.small ? `<small>${esc(it.small)}</small>` : ""}</span><span class="pal-r">${esc(it.right || "")}</span></div>`;
  });
  $("#pal-list").innerHTML = h || `<div class="pal-empty">Nothing matches "${esc(q)}".</div>`;
  $("#pal-q").setAttribute("aria-activedescendant", palItems.length ? `pal-${palSel}` : "");
  $(`#pal-${palSel}`)?.scrollIntoView({ block: "nearest" });
}
function openPalette() {
  closeOverlays();
  $("#pal-q").value = ""; palSel = 0; renderPalette();
  $("#palette").showModal(); $("#pal-q").focus();
}
function runPal(i) { const it = palItems[i]; if (!it) return; $("#palette").close(); it.run(); }
$("#open-palette").onclick = openPalette;
$("#pal-q").addEventListener("input", () => { palSel = 0; renderPalette(); });
$("#pal-q").addEventListener("keydown", e => {
  if (e.key === "ArrowDown") { e.preventDefault(); palSel = (palSel + 1) % Math.max(1, palItems.length); renderPalette(); }
  else if (e.key === "ArrowUp") { e.preventDefault(); palSel = (palSel - 1 + palItems.length) % Math.max(1, palItems.length); renderPalette(); }
  else if (e.key === "Enter") { e.preventDefault(); runPal(palSel); }
});
$("#pal-list").addEventListener("click", e => { const it = e.target.closest("[data-i]"); if (it) runPal(+it.dataset.i); });
$("#pal-list").addEventListener("mousemove", e => { const it = e.target.closest("[data-i]"); if (it && +it.dataset.i !== palSel) { palSel = +it.dataset.i; $$(".pal-item").forEach(x => x.setAttribute("aria-selected", x.dataset.i == palSel)); } });
$("#palette").addEventListener("click", e => { if (e.target.id === "palette") $("#palette").close(); });
$("#open-help").onclick = () => { closeOverlays(); $("#helpdlg").showModal(); };

function closeOverlays() {
  $$("dialog[open]").forEach(d => d.close());
  if (!$("#panel").hidden && route.view !== "project") showPanel(false);
  if (!$("#switcher").hidden) { $("#switcher").hidden = true; $("#switch-btn")?.setAttribute("aria-expanded", "false"); }
  $("#outbox").hidden = true;
}

/* ---------- stage chrome ---------- */
function approvalState(stageId) {
  const target = APPROVAL_TARGET[stageId];
  if (!target) return null;
  if ((D.stale || []).some(x => x.target === target)) return "stale";
  return (S.approvals || []).some(a => a.target === target) ? "done" : null;
}
function revOf(target) {
  return { brief: S.brief?.rev, direction: S.chosen_direction?.rev, dramaturgy: S.dramaturgy?.rev, storyboard: S.storyboard?.rev }[target] ?? 0;
}
function head(title, sub, stageId, extra = "") {
  const target = APPROVAL_TARGET[stageId];
  let ap = "";
  if (target) {
    const rev = revOf(target);
    const st = approvalState(stageId);
    if (!rev) ap = `<span class="chip">nothing to approve yet</span>`;
    else {
      const chip = st === "stale" ? `<span class="chip warn">approval stale</span>` : st === "done" ? `<span class="chip good">approved rev ${rev}</span>` : `<span class="chip">not approved</span>`;
      ap = `${chip}<button class="btn ${st === "done" ? "" : "primary"}" data-approve="${target}" data-rev="${rev}" ${st === "done" ? "disabled" : ""}>Approve rev ${rev}</button>`;
    }
  }
  return `<div class="stage-head"><div><div class="label">Stage ${STAGES.find(s => s.id === stageId).n}</div><h2>${title}</h2>${sub ? `<p>${sub}</p>` : ""}</div><div class="actions">${extra}${ap}</div></div>`;
}
function stageEmpty(title, text) { return `<div class="stage-empty"><div class="t">${title}</div><p>${text}</p></div>`; }
document.addEventListener("click", e => {
  const a = e.target.closest("[data-approve]");
  if (a && !a.disabled) send("approve", { target: a.dataset.approve, rev: +a.dataset.rev });
});
function renderShellBits() {
  const c = D.card;
  const stale = D.stale || [];
  $("#stale").classList.toggle("hidden", !stale.length);
  $("#stale").innerHTML = stale.map(s => `<span>Approval of <b>${esc(s.target)}</b> is stale: approved at rev ${s.approved_rev}, the ${esc(s.target)} is now at rev ${s.current_rev}. Look again and approve.</span>`).join("<br>");
  const arch = $("#archived-banner");
  arch.classList.toggle("hidden", c.status !== "archived");
  if (c.status === "archived") {
    arch.innerHTML = `<span>This project is archived. It stays on disk with all its files; it is only hidden from the active list.</span>${single ? "" : `<button class="btn small" id="unarch-b">Unarchive</button>`}`;
    const b = $("#unarch-b"); if (b) b.onclick = () => setArchived(c.slug, false);
  }
  const locks = S.locks || [];
  const nMust = locks.filter(l => l.kind === "must").length;
  $("#locks-sum").textContent = locks.length ? `Locks: ${nMust} must, ${locks.length - nMust} prefer` : "No locks yet";
  if (!renderShellBits.opened) { $("#locks").open = !phone.matches; renderShellBits.opened = true; }
  $("#locks-chips").innerHTML = locks.map(l => `<span class="chip ${l.kind}" title="${esc(l.kind)} · ${esc(l.scope)}${l.beat ? " " + esc(l.beat) : ""}"><b>${l.kind === "must" ? "must" : "prefer"}</b>${esc(l.text)}${l.scope !== "project" ? ` <span class="mono">${esc(l.beat || l.scope)}</span>` : ""}</span>`).join("");
  $("#locks").classList.toggle("hidden", selected === "brief" || !locks.length);
  $$("section.stage").forEach(sec => sec.classList.toggle("on", sec.dataset.stage === selected));
}

/* ---------- 0 mode ---------- */
function renderMode(el) {
  const deliberate = S.project.mode === "deliberate";
  el.innerHTML = head("Mode", "Quick renders straight away. Deliberate walks brief, directions, dramaturgy, storyboard and hero variants first, each ending in your choice.", "mode") + `
  <div class="grid g2">
    <div class="card pad">
      <div class="label">The one-line offer</div>
      <p class="serif" style="font-size:26px;line-height:1.25;margin:10px 0 0">"Render it straight away, or shall we decide the look, the arc and the hero moment together first?"</p>
    </div>
    <div class="card pad">
      <div class="label">Chosen</div>
      <div style="display:flex;gap:10px;margin-top:12px">
        <span class="chip ${deliberate ? "" : "accent"}">Quick</span><span class="chip ${deliberate ? "accent" : ""}">Deliberate</span>
      </div>
      <p class="muted" style="margin:14px 0 0">${S.project.source ? `Source <span class="mono">${esc(S.project.source)}</span>. ` : "No source yet. "}${esc(S.project.notes || "")}</p>
    </div>
  </div>`;
}

/* ---------- 1 brief ---------- */
function srcChip(s) { return s === "answered" ? `<span class="chip good">answered</span>` : s === "default-accepted" ? `<span class="chip">default accepted</span>` : `<span class="chip warn">open</span>`; }
function renderBrief(el) {
  if (ui.briefEdit && el.querySelector("[data-brief-form]")) return; // keep an open editor through live refreshes
  const b = S.brief || {};
  const val = (v, empty = "Not discussed yet") => v ? esc(v) : `<span class="muted">${empty}</span>`;
  const row = (k, v, src, field) => `<div class="commit${field ? " editable" : ""}"${field ? ` data-brief-field="${field}"` : ""}><div class="k">${k}</div><div class="v">${v}</div><div class="side">${srcChip(src)}${field ? `<button class="btn small ghost" data-brief-edit="${field}" aria-label="Edit ${k}">Edit</button>` : ""}</div></div>`;
  const content = b.content || { must: [], proposed: [] };
  const contentHtml = (content.must || []).length ? `<ul>${content.must.map(x => `<li>${esc(x)}</li>`).join("")}</ul>${(content.proposed || []).length ? `<div class="muted" style="margin-top:6px">Proposed, not supplied: ${content.proposed.map(esc).join("; ")}</div>` : ""}` : val("");
  const env = b.envelope || {};
  const taste = b.taste || {};
  const tasteHtml = `${val(taste.value)}${(taste.references || []).map(r => `<div class="muted" style="margin-top:6px"><span class="mono">${esc(r.src)}</span><br>borrow: ${esc(r.borrow)}<br>ignore: ${esc(r.ignore)}</div>`).join("")}`;
  const locks = (S.locks || []).map(l => `<div class="commit" style="grid-template-columns:70px 1fr auto"><span class="chip ${l.kind}">${l.kind}</span><span>${esc(l.text)}</span><span class="mono muted">${esc(l.scope)}${l.beat ? " " + esc(l.beat) : ""}</span></div>`).join("");
  const dialogue = (b.dialogue || []).map(m => `<div class="msg ${m.by}"><div class="who">${m.by} · turn ${m.turn}</div>${esc(m.text)}</div>`).join("");
  el.innerHTML = head("Brief", "Five commitments. Defaults are proposed by the agent and accepted with one word; open points stay provisional.", "brief") + `
  <div class="grid g2">
    <div class="card pad">
      ${row("Audience", val(b.audience?.value), b.audience?.source, "audience")}
      ${row("Outcome", val(b.outcome?.value), b.outcome?.source, "outcome")}
      ${row("Required content", contentHtml, content.source, "content")}
      ${row("Length", `${env.length_s ?? "?"} s`, env.source, "length_s")}
      ${row("Formats", `${(env.aspect || []).join(", ") || "?"} · ${env.fps ?? 30} fps`, env.source, "aspect")}
      ${row("Sound", val(env.sound), env.source, "sound")}
      ${row("Taste anchor", tasteHtml, taste.source, "taste")}
      ${row("Priority moment", `${val(b.priority?.value)} <span class="mono muted">${esc(b.priority?.beat || "")}</span>`, b.priority?.source, "priority")}
      ${(b.open || []).length ? row("Still open", `<ul>${b.open.map(x => `<li>${esc(x)}</li>`).join("")}</ul>`, "open") : ""}
    </div>
    <div class="grid" style="align-content:start">
      <div class="card pad"><div class="label" style="margin-bottom:8px">Locks</div>${locks || `<p class="muted" style="margin:0">None yet. Fixed points from the dialogue become must or prefer locks.</p>`}</div>
      <div class="card pad"><div class="label" style="margin-bottom:12px">How we got here</div><div class="dialogue">
        ${dialogue || `<p class="muted" style="margin:0">The dialogue has not started. The agent asks two or three questions per turn, each with a default.</p>`}
      </div></div>
    </div>
  </div>`;
  $$("[data-brief-edit]", el).forEach(btn => btn.onclick = () => openBriefEditor(el, btn.dataset.briefEdit));
}
function briefRaw(field) {
  const b = S.brief || {}, env = b.envelope || {}, c = b.content || {};
  if (field === "content") return [...(c.must || []), ...(c.proposed || [])].join("\n");
  if (field === "length_s") return String(env.length_s ?? "");
  if (field === "aspect") return (env.aspect || []).join(", ");
  if (field === "sound") return env.sound || "";
  return b[field]?.value || "";
}
const BRIEF_HINT = { content: "One item per line. Saved items become required content.", length_s: "Seconds, e.g. 30", aspect: "e.g. 16:9, 9:16" };
function openBriefEditor(el, field) {
  const rowEl = $(`[data-brief-field="${field}"]`, el);
  if (!rowEl) return;
  ui.briefEdit = field;
  const short = field === "length_s" || field === "aspect";
  const v = $(".v", rowEl);
  v.innerHTML = `<form data-brief-form class="brief-form">
    ${short ? `<input type="text" name="value" class="input" value="${esc(briefRaw(field))}" autocomplete="off">` : `<textarea name="value" class="input" rows="${field === "content" ? 5 : 3}">${esc(briefRaw(field))}</textarea>`}
    ${BRIEF_HINT[field] ? `<div class="muted small">${BRIEF_HINT[field]}</div>` : ""}
    <div class="acts"><button class="btn primary small" type="submit">Save</button><button class="btn small ghost" type="button" data-cancel>Cancel</button><span class="muted small">Ctrl+Enter saves, Esc cancels</span></div></form>`;
  $(".side", rowEl).hidden = true;
  const form = $("form", v), input = form.elements.value;
  input.focus(); input.setSelectionRange?.(input.value.length, input.value.length);
  const close = () => { ui.briefEdit = null; renderBrief(el); };
  form.onsubmit = async e => {
    e.preventDefault();
    try {
      const j = await api(`/api/p/${encodeURIComponent(slug)}/brief`, { field, value: input.value });
      toast(`Saved. Brief is now rev ${j.rev}; the agent sees the change.`);
      ui.briefEdit = null;
      if (await loadProject(false)) draw(false);
    } catch (err) { toast("Not saved: " + err.message); }
  };
  $("[data-cancel]", form).onclick = close;
  form.onkeydown = e => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); form.requestSubmit(); }
  };
}

/* ---------- 2 directions ---------- */
const ATTRS = ["palette", "type", "texture", "motion", "music"];
function renderDirections(el) {
  const ch = S.chosen_direction || {};
  const dirs = S.directions || [];
  const sub = "Three genuinely different directions, one style frame each, same copy so they compare. Pick one, mix attributes, or ask for a new one. Rejected directions stay here.";
  if (!dirs.length) { el.innerHTML = head("Directions", sub, "directions") + stageEmpty("No directions yet", "After the brief is approved the agent renders three style frames that differ in concept and material, not just colour."); return; }
  if (!ui.dfocus || !dirs.some(d => d.id === ui.dfocus)) ui.dfocus = (dirs.find(d => d.status === "chosen") || dirs[0]).id;
  const mixFrom = a => (ch.mix || []).find(m => m.attribute === a)?.from || ch.base || dirs[0]?.id;
  el.innerHTML = head("Directions", sub, "directions") + `
  <div class="grid g3">${dirs.map((d, i) => `
    <article class="card dir ${esc(d.status)} ${d.id === ui.dfocus ? "kfocus" : ""}" data-d="${esc(d.id)}">
      <div class="frame">${d.frame ? pointable(d.frame, `<img loading="lazy" src="${media(d.frame)}" alt="Style frame ${esc(d.name)}">`, { label: "style " + d.id }) : ""}
        <span class="badge ${d.status === "chosen" ? "waiting" : "quiet"}">${esc(d.id)} · ${esc(d.status)}</span>${i < 3 ? `<span class="keyhint screen-key" aria-hidden="true">${i + 1}</span>` : ""}</div>
      <div class="pad">
        <h3>${esc(d.name)}</h3>
        <p class="idea">${esc(d.idea)}</p>
        <div class="swatches">${(d.palette || []).map(c => `<i style="background:${esc(c)}" title="${esc(c)}"></i>`).join("")}</div>
        <dl class="attrs"><dt>Type</dt><dd>${esc(d.type)}</dd><dt>Texture</dt><dd>${esc(d.texture)}</dd><dt>Motion</dt><dd>${esc(d.motion)}</dd><dt>Music</dt><dd>${esc(d.music)}</dd></dl>
        ${(d.conflicts || []).map(c => `<div class="chip warn" style="margin-top:12px;white-space:normal">conflicts with ${esc(c)}</div>`).join("")}
        ${d.rejected_note ? `<p class="quote" style="margin:14px 0 0">"${esc(d.rejected_note)}"</p>` : ""}
      </div>
      <div class="foot">
        <button class="btn ${d.status === "chosen" ? "" : "primary"} small" data-pickdir="${esc(d.id)}">${d.status === "chosen" ? "Picked" : "Pick " + esc(d.id)}</button>
        <span class="mono muted" style="margin-left:auto">${esc(d.source)}</span>
      </div>
    </article>`).join("")}
  </div>
  <div class="card pad" style="margin-top:16px">
    <div style="display:flex;gap:12px;align-items:baseline;flex-wrap:wrap;margin-bottom:12px">
      <div class="label">Mix</div><span class="muted">Current choice: <b>${esc(ch.note || ch.base || "none")}</b></span>
    </div>
    <div class="mixer">
      ${ATTRS.map(a => `<label class="field"><span class="label">${a}</span><select data-mix="${a}">${dirs.map(d => `<option value="${esc(d.id)}" ${mixFrom(a) === d.id ? "selected" : ""}>${esc(d.id)} · ${esc(d.name)}</option>`).join("")}</select></label>`).join("")}
      <button class="btn" id="send-mix">Send mix</button>
    </div>
    <div class="inline-send" style="margin-top:16px">
      <input type="text" id="newdir" data-note placeholder="None of these. A new direction: what should change in concept, not just colour?">
      <button class="btn" id="send-newdir">New direction</button>
    </div>
  </div>
  <div class="hint-row"><span><kbd>J</kbd><kbd>K</kbd> move</span><span><kbd>1</kbd><kbd>2</kbd><kbd>3</kbd> pick</span><span><kbd>N</kbd> new direction</span></div>`;
  $$("[data-pickdir]", el).forEach(b => b.onclick = () => pickDirection(b.dataset.pickdir));
  $$(".dir", el).forEach(c => c.addEventListener("click", e => { if (!e.target.closest("button")) focusDir(c.dataset.d); }));
  $("#send-mix", el).onclick = () => {
    const attributes = Object.fromEntries($$("[data-mix]", el).map(s => [s.dataset.mix, s.value]));
    send("mix", { target: { type: "direction" }, attributes, text: ATTRS.map(a => `${a} of ${attributes[a]}`).join(", ") });
  };
  const sendNew = () => { const v = $("#newdir", el).value.trim(); if (!v) return toast("Say what should be different"); send("new-direction", { target: { type: "direction" }, text: v }); $("#newdir", el).value = ""; };
  $("#send-newdir", el).onclick = sendNew;
  $("#newdir", el).addEventListener("keydown", e => { if (e.key === "Enter") sendNew(); });
}
function pickDirection(id) { send("pick", { target: { type: "direction", id } }); }
function focusDir(id) { ui.dfocus = id; $$(".dir").forEach(c => c.classList.toggle("kfocus", c.dataset.d === id)); }

/* ---------- 3 dramaturgy ---------- */
const PART_COLORS = { hook: "#7d8cff", turn: "#ff80bf", tension: "#80ffea", payoff: "#ffca80", cta: "#cbbcff" };
function partOf(id) { return ((S.dramaturgy || {}).arc || []).find(a => a.beats.includes(id))?.part || "beat"; }
function renderDramaturgy(el) {
  const dr = S.dramaturgy || {}, beats = S.beats || [], env = S.brief?.envelope?.length_s;
  const sub0 = "The arc, a beat list on the music grid, and an energy curve with one peak: the hero moment.";
  if (!beats.length || !dr.rev) { el.innerHTML = head("Dramaturgy", sub0, "dramaturgy") + stageEmpty("No beat list yet", "Once a direction is chosen the agent proposes the shape, beats with durations that sum to the delivery length, and the hit points on the music grid."); return; }
  const total = beats.reduce((s, b) => s + b.dur, 0);
  const music = dr.music || {};
  const W = 1000, H = 150, pad = 12;
  const x = t => (t / total) * W;
  const y = e => H - pad - e * (H - 2 * pad);
  let pts = [], t = 0;
  beats.forEach(b => { pts.push([x(t), y(b.energy[0])]); pts.push([x(t + b.dur), y(b.energy[1])]); t += b.dur; });
  const path = pts.map((p, i) => `${i ? "L" : "M"}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(" ");
  const area = path + ` L${W} ${H} L0 ${H} Z`;
  const hits = music.hits || [];
  const gridLines = [];
  if (music.bar_s) for (let bt = 0; bt <= total + 1e-6; bt += music.bar_s) gridLines.push(`<line x1="${x(bt)}" x2="${x(bt)}" y1="0" y2="${H}" stroke="currentColor" stroke-opacity="${Math.round(bt / music.bar_s) % 4 === 0 ? .22 : .08}"/>`);
  el.innerHTML = head("Dramaturgy", `${esc(dr.shape)}. ${beats.length} beats, ${total.toFixed(3).replace(/\.?0+$/, "")} s of ${env} s. Hero: ${esc(dr.hero)}.`, "dramaturgy",
    Math.abs(total - env) > 0.01 ? `<span class="chip warn">sum ${total.toFixed(2)} s, envelope ${env} s</span>` : `<span class="chip good">sums to ${env} s</span>`) + `
  <div class="card pad">
    <div class="arc" style="margin-bottom:16px">${(dr.arc || []).map(a => `<div class="part"><div class="label" style="color:${PART_COLORS[a.part] || "inherit"}">${esc(a.part)}</div><div class="mono">${a.beats.join(" ")}</div></div>`).join("")}</div>
    <div class="timeline">
      <div class="gridboard"><div class="gb-inner">
        ${music.bar_s ? `<div class="bars" aria-hidden="true">${Array.from({ length: Math.floor(total / music.bar_s + 1e-6) + 1 }, (_, k) => `<span class="${k % 4 === 0 ? "major" : ""}" style="left:${(k * music.bar_s / total * 100).toFixed(3)}%">${k % (total / music.bar_s > 24 ? 4 : 1) === 0 ? k : ""}</span>`).join("")}</div>` : ""}
        <div class="tl-beats board-grid">${beats.map(b => `<button class="gb-beat ${b.hero ? "hero" : ""}" data-beat-pick="${esc(b.id)}" style="flex:${b.dur};--pc:${PART_COLORS[partOf(b.id)] || "#888"}" title="${esc(b.title)}: ${b.dur} s, bars ${esc(b.bars || "")}. Click to use as context.">
          <span class="gb-still">${b.still ? `<img src="${media(b.still)}" alt="" loading="lazy">` : ""}</span>
          <span class="gb-txt"><b>${esc(b.id)}</b> ${esc(b.title)}</span><span class="gb-bars mono">${b.bars ? "bars " + esc(b.bars) : b.dur + " s"}</span></button>`).join("")}</div>
      </div></div>
      <div class="energy" style="color:var(--muted)">
        <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-label="Energy curve">
          ${gridLines.join("")}
          <path d="${area}" fill="var(--accent)" fill-opacity=".12"/>
          <path d="${path}" fill="none" stroke="var(--accent)" stroke-width="2" vector-effect="non-scaling-stroke"/>
          ${hits.map(h => `<line x1="${x(h.t)}" x2="${x(h.t)}" y1="0" y2="${H}" stroke="var(--ink)" stroke-width="1.5" stroke-dasharray="3 3" vector-effect="non-scaling-stroke"/>`).join("")}
        </svg>
      </div>
      <div style="display:flex;justify-content:space-between;gap:12px" class="mono muted"><span>0 s</span><span>energy per beat${music.bar_s ? `, bar lines every ${music.bar_s} s` : ""}</span><span>${total} s</span></div>
    </div>
  </div>
  <div class="grid g2" style="margin-top:16px">
    <div class="card pad">
      <div class="label" style="margin-bottom:10px">Beats</div>
      ${beats.map(b => `<div class="commit" style="grid-template-columns:44px 1fr auto"><span class="mono">${esc(b.id)}</span><span><b>${esc(b.title)}</b> ${b.hero ? `<span class="chip accent">hero</span>` : ""}<br><span class="muted">${esc(b.intent)}</span></span><span class="mono muted" style="text-align:right;white-space:nowrap">${b.t0.toFixed(2)} s<br>+${b.dur} s<br>bars ${esc(b.bars)}</span></div>`).join("")}
    </div>
    <div class="card pad" style="align-self:start">
      <div class="label" style="margin-bottom:10px">Music plan</div>
      <p style="margin:0 0 10px">${music.bpm ? `${music.bpm} bpm · bar ${music.bar_s} s · beat ${music.beat_s} s` : "No grid yet"}<br><span class="muted">${esc(music.notation)}</span></p>
      <div class="hits">${hits.map(h => `<span class="mono">${esc(h.at)}</span><span class="mono muted">${h.t} s</span><span>${esc(h.what)}</span>`).join("")}</div>
      <div class="inline-send" style="margin-top:16px">
        <input type="text" id="drnote" data-note placeholder="A note on the arc, e.g. move the hero one bar later">
        <button class="btn" id="send-drnote">Send note</button>
      </div>
    </div>
  </div>`;
  const sendNote = () => { const v = $("#drnote", el).value.trim(); if (!v) return; send("note", { text: v, beat: null }); $("#drnote", el).value = ""; };
  $("#send-drnote", el).onclick = sendNote;
  $("#drnote", el).addEventListener("keydown", e => { if (e.key === "Enter") sendNote(); });
}

/* ---------- 4 storyboard ---------- */
function renderStoryboard(el) {
  const beats = S.beats || [];
  const sub = "One still per beat, the motion in a line, and a menu of animation kinds reel can actually render. Card width does not mean duration; the label does.";
  if (!beats.length) { el.innerHTML = head("Storyboard", sub, "storyboard") + stageEmpty("No storyboard yet", "After the dramaturgy is locked the agent renders one still per beat and offers two to four animation kinds per beat."); return; }
  const byId = Object.fromEntries(beats.map(b => [b.id, b]));
  const order = ui.order && ui.order.length === beats.length ? ui.order : beats.map(b => b.id);
  const changed = order.join() !== beats.map(b => b.id).join();
  el.innerHTML = head("Storyboard", sub, "storyboard",
    `<button class="btn ${changed ? "primary" : ""}" id="send-order" ${changed ? "" : "disabled"}>Send new order</button>${changed ? `<button class="btn ghost" id="reset-order">Reset</button>` : ""}`) + `
  <div class="board">${order.map((id, i) => { const b = byId[id]; return `
    <article class="card beat">
      <div class="still">${b.still ? pointable(b.still, `<img loading="lazy" src="${media(b.still)}" alt="${esc(b.title)}">`, { beat: b.id, label: b.id + " " + b.title }) : `<span class="nostill">no still yet</span>`}<span class="num">${String(i + 1).padStart(2, "0")}</span>${b.hero ? `<span class="hero-tag badge waiting">hero</span>` : ""}</div>
      <div class="body">
        <h3>${esc(b.title)} <span class="mono">${b.dur} s</span></h3>
        <p class="intent">${esc(b.intent)}</p>
        <p class="note">${esc(b.motion_note)}</p>
        ${(b.kinds || []).length ? `<div class="label" style="margin-top:4px">Animation kind</div>
        <div class="kinds">${b.kinds.map(k => `<button class="chip kind ${k.id === b.kind ? "on" : ""} ${k.status === "planned" ? "planned" : ""}" data-beat="${esc(b.id)}" data-kind="${esc(k.id)}" ${k.status === "planned" ? "disabled" : ""} aria-pressed="${k.id === b.kind}" title="${esc(k.syntax)} (${esc(k.status)})">${esc(k.label)}${k.status !== "implemented" ? ` <span class="mono">${esc(k.status)}</span>` : ""}</button>`).join("")}</div>` : ""}
      </div>
      <div class="foot">
        <span class="status ${esc(b.status)}">${esc(b.status)}</span>
        <button class="chip beatchip" data-beat-pick="${esc(b.id)}" title="Use this beat as context">${esc(b.id)} · ${b.t0.toFixed(2)} s</button>
        <span style="margin-left:auto;display:flex;gap:4px">
          <button class="btn small ghost" data-move="${esc(id)}" data-dir="-1" ${i === 0 ? "disabled" : ""} aria-label="Move ${esc(b.title)} earlier">&larr;</button>
          <button class="btn small ghost" data-move="${esc(id)}" data-dir="1" ${i === order.length - 1 ? "disabled" : ""} aria-label="Move ${esc(b.title)} later">&rarr;</button>
        </span>
      </div>
    </article>`; }).join("")}
  </div>
  ${(S.notes || []).length ? `<div class="card pad" style="margin-top:16px"><div class="label" style="margin-bottom:8px">Notes</div>${S.notes.map(n => `<div class="commit" style="grid-template-columns:52px 1fr auto"><span class="mono">${esc(n.beat || "")}</span><span>${esc(n.text)} <span class="muted mono">@ ${n.t} s ${esc(n.clock || "")} · rev ${n.rev}</span></span><span class="chip ${n.status === "done" ? "good" : ""}">${esc(n.status)}${n.resolved_rev ? " rev " + n.resolved_rev : ""}</span></div>`).join("")}</div>` : ""}`;
  $$("[data-kind]", el).forEach(b => b.onclick = () => send("pick", { target: { type: "kind", beat: b.dataset.beat, kind: b.dataset.kind } }));
  $$("[data-move]", el).forEach(b => b.onclick = () => {
    const o = order.slice(), i = o.indexOf(b.dataset.move), j = i + +b.dataset.dir;
    [o[i], o[j]] = [o[j], o[i]]; ui.order = o; keys.storyboard = null; renderStoryboard(el);
    $(`[data-move="${b.dataset.move}"][data-dir="${b.dataset.dir}"]`, el)?.focus();
  });
  const so = $("#send-order", el); if (so) so.onclick = () => send("reorder", { order });
  const ro = $("#reset-order", el); if (ro) ro.onclick = () => { ui.order = null; renderStoryboard(el); };
}

/* ---------- 5 variants ---------- */
let vplayer = null;
function renderVariants(el) {
  const vs = S.variants || [];
  const sub0 = "Three short clips of the hero beat that differ on one named axis, played in sync.";
  const prev = vplayer ? vplayer.state() : null;
  if (vplayer) { vplayer.stop(); vplayer = null; }
  if (!vs.length) { el.innerHTML = head("Hero variants", sub0, "variants") + stageEmpty("No variants yet", "When the storyboard is approved the agent writes three clips of the hero beat that differ on one axis, with the cue aligned so you can compare them in sync."); return; }
  const beat = (S.beats || []).find(b => b.id === vs[0]?.beat) || {};
  if (!ui.focus || !vs.some(v => v.id === ui.focus)) ui.focus = vs[0]?.id;
  const axis = vs[0]?.axis || "";
  const clips = vs.map((v, i) => ({ id: v.id, tag: "ABCDEFG"[i], src: media(v.clip), poster: media(v.still), cue: v.cue, dur: v.dur, clip_t0: v.clip_t0 }));
  el.innerHTML = head("Hero variants", `Beat ${esc(beat.id)} "${esc(beat.title)}". Three clips that differ on one axis: <b>${esc(axis)}</b>. Everything else is locked.`, "variants",
    `<span class="chip accent">axis: ${esc(axis)}</span>`) +
  Sync.html(clips, {
    focus: ui.focus,
    body: c => { const v = vs.find(x => x.id === c.id); return `<div class="body"><h3>${esc(v.value)} <span class="mono">${v.dur} s · cue ${v.cue} s</span></h3><p class="diff">${esc(v.diff)}</p>
      <div class="mono muted">${esc(v.id)}${v.parent ? " ← " + esc(v.parent) : " (baseline)"} · ${esc(v.source)}</div></div>`; },
    foot: c => { const v = vs.find(x => x.id === c.id); return `<button class="btn small ${v.status === "chosen" ? "" : "primary"}" data-pick="${esc(v.id)}">${v.status === "chosen" ? "Picked" : "Pick"}</button>
      <button class="btn small" data-branch="${esc(v.id)}">Branch</button>
      <label class="chip mixtick"><input type="checkbox" data-mixsel="${esc(v.id)}" ${ui.picked.has(v.id) ? "checked" : ""}> mix</label>`; },
  }) + `
  <div class="card notebar">
    <span class="mono" id="vnote-at">note on ${esc(ui.focus)} at 0.00 s</span>
    <input type="text" id="vnote" data-note placeholder="A note at this moment or range, e.g. keep the gathering, remove the bounce">
    <button class="btn" id="vnote-send">Add note</button>
  </div>
  <div class="card pad vmixrow">
    <input type="text" id="vmix" placeholder="Mix the ticked clips: what to keep from each (e.g. the timing of A, the ink of B)">
    <button class="btn" id="vmix-send">Combine ticked</button>
    <button class="btn ghost" id="vnew">None of these</button>
  </div>
  <div class="hint-row"><span><kbd>Space</kbd> play</span><span><kbd>J</kbd><kbd>K</kbd> focus</span><span><kbd>1</kbd><kbd>2</kbd><kbd>3</kbd> pick</span><span><kbd>I</kbd><kbd>O</kbd> range</span><span><kbd>N</kbd> note at this moment</span><span>All clips play muted; solo one to hear the score.</span></div>
  <div class="notes-list">${(S.notes || []).filter(n => n.beat === beat.id).map(n => `<div class="n"><span class="mono">${esc(n.variant || "")} ${n.t0 != null ? rangeTxt(n.t0, n.t1) : "@ " + n.t + " s"}</span><span>${esc(n.text)}</span></div>`).join("")}</div>`;
  vplayer = new Sync($(".sync", el), clips, {
    focus: ui.focus,
    on: {
      focus: id => { ui.focus = id; },
      time: at => { const l = $("#vnote-at"); if (l) l.textContent = `note on ${at.clip} ${at.t0 != null ? rangeTxt(at.t0, at.t1) : "at " + secs(at.t) + " s"}`; },
      range: r => setCtx({ range: r ? { ...r, src: (vs.find(v => v.id === r.clip) || {}).clip, label: r.clip } : null }),
    },
  });
  vplayer.restore(prev);
  $$("[data-pick]", el).forEach(b => b.onclick = () => pickVariant(b.dataset.pick));
  $$("[data-branch]", el).forEach(b => b.onclick = async () => {
    const text = await ask(`Branch ${b.dataset.branch}`, "A child of this clip. Change only one thing; everything else stays.", "e.g. land the signet half a beat later");
    if (text) send("branch", { parent: b.dataset.branch, beat: beat.id, text });
  });
  $$("[data-mixsel]", el).forEach(c => c.onchange = () => { c.checked ? ui.picked.add(c.dataset.mixsel) : ui.picked.delete(c.dataset.mixsel); });
  $("#vmix-send", el).onclick = () => {
    const text = $("#vmix", el).value.trim();
    if (ui.picked.size < 2) return toast("Tick at least two clips to mix");
    if (!text) return toast("Say what to keep from each");
    send("mix", { target: { type: "variant", beat: beat.id }, variants: [...ui.picked], text }); $("#vmix", el).value = "";
  };
  $("#vnew", el).onclick = async () => {
    const text = await ask("None of these", `What should a new treatment of "${beat.title}" do? It still varies only ${axis}.`, "e.g. the list folds away like paper");
    if (text) send("new-direction", { target: { type: "variant", beat: beat.id, axis }, text });
  };
  const sendNote = () => {
    const text = $("#vnote", el).value.trim(); if (!text) return toast("Write the note first");
    const { clip, ...at } = vplayer.at();
    send("note", { variant: clip, beat: beat.id, ...at, clock: "clip", text });
    $("#vnote", el).value = "";
  };
  $("#vnote-send", el).onclick = sendNote;
  $("#vnote", el).addEventListener("keydown", e => { if (e.key === "Enter") sendNote(); });
}
function pickVariant(id) { const v = (S.variants || []).find(x => x.id === id); if (v) send("pick", { target: { type: "variant", id, beat: v.beat } }); }

/* ---------- 6 render ---------- */
function renderRender(el) {
  const r = S.render || {};
  el.innerHTML = head("First full render", "After the hero is picked: masters, a contact sheet, then the normal review loop with notes at a time and a beat.", "render") + `
  ${(r.outputs || []).length ? `<div class="grid g2">${r.outputs.map(o => `<div class="card pad"><div class="label">${esc(o.label || o.format || "")}</div>${/\.mp4$/.test(o.path) ? `<video controls src="${media(o.path)}" style="margin-top:10px;width:100%"></video>` : `<img src="${media(o.path)}" style="margin-top:10px" alt="${esc(o.label || "")}">`}</div>`).join("")}</div>`
    : stageEmpty("Not rendered yet", `${esc(r.planned || "The whole film renders only after the hero variant is picked.")}<br><span class="mono">status: ${esc(r.status || "not-started")}</span>`)}`;
}

/* ---------- draw ---------- */
const RENDERERS = { mode: renderMode, brief: renderBrief, directions: renderDirections, dramaturgy: renderDramaturgy, storyboard: renderStoryboard, variants: renderVariants, render: renderRender };
const SLICES = {
  mode: () => [S.project], brief: () => [S.brief, S.locks, D.stale, S.approvals], directions: () => [S.directions, S.chosen_direction, D.stale, S.approvals],
  dramaturgy: () => [S.dramaturgy, S.beats, D.stale, S.approvals], storyboard: () => [S.beats, S.notes, S.storyboard, D.stale, S.approvals],
  variants: () => [S.variants, S.notes], render: () => [S.render],
};
function draw(force) {
  if (!S || route.view !== "project") return;
  renderCrumbs(); renderProjectHead(); renderShellBits(); renderStream(); renderCtx();
  if (!$("#outbox").hidden) renderOutbox();
  if (!selected) return;
  // only the open panel is built; the others are drawn when opened
  const id = selected;
  const key = JSON.stringify(SLICES[id]()) + mediaV;
  if (!force && keys[id] === key) return;
  keys[id] = key;
  const sec = $(`section[data-stage="${id}"]`);
  try { RENDERERS[id](sec); renderCtx(); }
  catch (e) { sec.innerHTML = stageEmpty(`Could not draw ${esc(id)}`, esc(e.message)); console.error(e); }
}

/* ---------- keyboard ---------- */
let gPending = 0;
document.addEventListener("keydown", e => {
  const typing = e.target.matches("input, textarea, select, [contenteditable]");
  const mod = e.metaKey || e.ctrlKey;
  if (mod && e.key.toLowerCase() === "k") { e.preventDefault(); $("#palette").open ? $("#palette").close() : openPalette(); return; }
  if (e.key === "Escape") {
    if (!$("#switcher").hidden) { toggleSwitcher(false); return; }
    if (!$("#outbox").hidden) { $("#outbox").hidden = true; $("#open-outbox").focus(); return; }
    if ($("dialog[open]")) return;
    if (typing) { e.target.blur(); return; }
    if (!$("#panel").hidden) { closePanel(); return; }
    if (!ctxEmpty()) clearCtx();
    return;
  }
  if (typing || mod || e.altKey || $("dialog[open]")) return;
  const k = e.key, kl = k.toLowerCase();
  const onControl = e.target.closest("button, a, summary, label");
  if (k === "?") { e.preventDefault(); $("#helpdlg").showModal(); return; }
  if (gPending && Date.now() - gPending < 900) { gPending = 0; if (kl === "h") { e.preventDefault(); go("/"); } return; }
  if (kl === "g" && !e.shiftKey) { gPending = Date.now(); return; }
  if (route.view === "home") {
    if (k === "/") { e.preventDefault(); $("#hq")?.focus(); }
    else if (kl === "c" && !single) { e.preventDefault(); openNew(); }
    return;
  }
  if (route.view !== "project" || !S) return;
  if (!$("#switcher").hidden) return;
  if (kl === "o" && e.shiftKey) { e.preventDefault(); openOutbox(); return; }
  const dig = /^Digit([1-9])$/.exec(e.code);
  if (e.shiftKey && dig && RAIL[+dig[1] - 1]) { e.preventDefault(); go(projectPath(slug, RAIL[+dig[1] - 1].id), { replace: !!selected }); return; }
  if (kl === "x") { e.preventDefault(); clearCtx(); return; }
  const next = ["j", "arrowright"].includes(kl) ? 1 : ["k", "arrowleft"].includes(kl) ? -1 : 0;

  if (selected) {        // a stage panel is open
    if (k === "[" || k === "]") { e.preventDefault(); (k === "]" ? $("#panel-next") : $("#panel-prev")).click(); return; }
    if (kl === "n") {
      const f = $(`section[data-stage="${selected}"] [data-note]`);
      if (f) { e.preventDefault(); f.focus(); f.scrollIntoView({ block: "center", behavior: reduceMotion.matches ? "auto" : "smooth" }); }
      return;
    }
    if (selected === "variants" && vplayer) {
      if (e.code === "Space" && !onControl) { e.preventDefault(); vplayer.toggle(); return; }
      if (kl === "i" || kl === "o") { e.preventDefault(); vplayer.mark(kl === "i" ? "in" : "out"); return; }
      const ids = vplayer.clips.map(c => c.id);
      if (next) { e.preventDefault(); vplayer.setFocus(ids[(ids.indexOf(vplayer.focus) + next + ids.length) % ids.length]); $(`[data-clip-card="${CSS.escape(vplayer.focus)}"]`)?.scrollIntoView({ block: "nearest", behavior: reduceMotion.matches ? "auto" : "smooth" }); return; }
      if (/^[1-9]$/.test(k) && ids[+k - 1]) { e.preventDefault(); vplayer.setFocus(ids[+k - 1]); pickVariant(ids[+k - 1]); return; }
    }
    if (selected === "directions" && (S.directions || []).length) {
      const ids = S.directions.map(d => d.id);
      if (next) { e.preventDefault(); focusDir(ids[(ids.indexOf(ui.dfocus) + next + ids.length) % ids.length]); $(`.dir[data-d="${ui.dfocus}"]`)?.scrollIntoView({ block: "nearest", behavior: reduceMotion.matches ? "auto" : "smooth" }); return; }
      if (/^[1-9]$/.test(k) && ids[+k - 1]) { e.preventDefault(); focusDir(ids[+k - 1]); pickDirection(ids[+k - 1]); return; }
    }
    return;
  }

  // the stream
  const open = openCards();
  const c = open.find(x => x.id === activeCard);
  if (kl === "j" || kl === "k") {
    if (!open.length) return;
    e.preventDefault();
    const i = open.findIndex(x => x.id === activeCard);
    setActive(open[Math.min(open.length - 1, Math.max(0, i + (kl === "j" ? 1 : -1)))].id, true);
    return;
  }
  if (!c) return;
  if (/^[1-4]$/.test(k) && (c.type === "choice" || c.type === "compare") && c.options[+k - 1]) { e.preventDefault(); selectOpt(c, c.options[+k - 1].id); return; }
  if (k === "Enter" && !onControl) { e.preventDefault(); submit(c); return; }
  if (kl === "n") {
    e.preventDefault();
    if (c.type === "approve" && !draft(c.id).changes) { draft(c.id).changes = true; rerenderCard(c); }
    const f = $(`.dcard[data-card="${CSS.escape(c.id)}"] [data-note]`);
    if (f) { f.focus(); f.scrollIntoView({ block: "center", behavior: reduceMotion.matches ? "auto" : "smooth" }); }
    return;
  }
  const p = players[c.id];
  if (p && e.code === "Space" && !onControl) { e.preventDefault(); p.toggle(); return; }
  if (p && (kl === "i" || kl === "o")) { e.preventDefault(); p.mark(kl === "i" ? "in" : "out"); return; }
});

/* ---------- start ---------- */
(async () => {
  await loadProjects();
  await onRoute();
  setInterval(poll, 2000);
  setInterval(() => { if (route.view === "home") $$(".pcard .upd").forEach((el, i) => { const p = filtered()[i]; if (p) el.textContent = ago(p.updated); }); }, 60000);
})();
