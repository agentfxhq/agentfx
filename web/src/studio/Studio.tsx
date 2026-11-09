// Studio: code editor + live preview + timeline + inspector, kept in sync through the JSON form.
//
// Typing:   text -> POST /parse (JSON) + /timing + /lint -> timeline, squiggles.
// Timeline: JSON -> edit -> POST /parse (text) -> patched into the author's text as one editor
//           transaction, so undo/redo covers both and comments survive (model/patch.ts).
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { api, ApiError, type Catalog, type Diagnostic } from '../api';
import { BASE_GROUPS, easeGroups } from '../model/easings';
import { patchSource } from '../model/patch';
import { formatSize, nativeOnlyFeatures, videoInfo, type ReelDoc } from '../model/reel';
import * as T from '../model/timeline';
import { Preview, type ExactMode } from '../preview/Preview';
import { Transport } from '../preview/Transport';
import { RenderPanel } from '../template/RenderPanel';
import { HANDOFF_KEY } from '../template/TemplateMode';
import { toast, useDebounced, useLatest } from '../ui/hooks';
import { Eye, Film, Redo, Undo, X } from '../ui/icons';
import { CodeEditor, type EditorApi } from './CodeEditor';
import { Diagnostics } from './Diagnostics';
import { DEMO } from './demo';
import { Inspector, type InspectorActions } from './Inspector';
import { Timeline, type Sel } from './Timeline';

interface Analysis {
  doc: ReelDoc;
  timing: T.Timing | null;
  src: string; // the text `doc` was parsed from
}

const canonCache = new WeakMap<ReelDoc, Promise<string>>();
const canon = (d: ReelDoc) => {
  let p = canonCache.get(d);
  if (!p) {
    p = api.toText(d);
    canonCache.set(d, p);
    p.catch(() => canonCache.delete(d));
  }
  return p;
};

export function Studio({ templateId }: { templateId: string | null }) {
  const editor = useRef<EditorApi>(null);
  const [initial, setInitial] = useState<string | null>(null);
  const [source, setSource] = useState('');
  const [format, setFormat] = useState('');
  const [analysis, setAnalysis] = useState<Analysis | null>(null);
  const [optimistic, setOptimistic] = useState<ReelDoc | null>(null);
  const [diags, setDiags] = useState<Diagnostic[]>([]);
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [sel, setSel] = useState<Sel>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [hist, setHist] = useState({ undo: 0, redo: 0 });
  const [exact, setExact] = useState<ExactMode>('auto');
  const [showRender, setShowRender] = useState(false);
  const [tlH, setTlH] = useState(300);
  const sourceRef = useLatest(source);
  const analysisRef = useLatest(analysis);

  // initial script: handed over from template mode, a template, or the demo
  useEffect(() => {
    let handoff: { source: string; format?: string } | null = null;
    try {
      handoff = JSON.parse(sessionStorage.getItem(HANDOFF_KEY) ?? 'null');
      sessionStorage.removeItem(HANDOFF_KEY);
    } catch {
      /* ignore */
    }
    if (handoff?.source) {
      setFormat(handoff.format ?? '');
      setInitial(handoff.source);
    } else if (templateId) api.instantiate(templateId, {}).then(setInitial, () => setInitial(DEMO));
    else setInitial(DEMO);
    api.catalog().then(setCatalog, () => {});
  }, []);
  useEffect(() => {
    if (initial !== null) setSource(initial);
  }, [initial]);

  // text -> JSON + timing + lint
  const dsrc = useDebounced(source, 220);
  useEffect(() => {
    if (!dsrc) return;
    const ac = new AbortController();
    const fmt = format || undefined;
    Promise.allSettled([api.toJson(dsrc, ac.signal), api.timing(dsrc, fmt, ac.signal), api.lint(dsrc, fmt, ac.signal)]).then(([j, tm, li]) => {
      if (ac.signal.aborted) return;
      if (j.status === 'fulfilled') {
        setAnalysis((a) => ({ doc: j.value, timing: tm.status === 'fulfilled' ? tm.value : a?.timing ?? null, src: dsrc }));
        setOptimistic(null);
      }
      if (li.status === 'fulfilled') setDiags(li.value);
      else if (j.status === 'rejected' && j.reason instanceof ApiError) setDiags(j.reason.diagnostics);
    });
    return () => ac.abort();
  }, [dsrc, format]);

  const doc = optimistic ?? analysis?.doc ?? null;
  const tl = useMemo(() => (doc ? T.buildTimeline(doc, analysis?.timing ?? null) : null), [doc, analysis?.timing]);
  const info = videoInfo(doc);
  const formats = info.formats.length ? info.formats : [`${info.width}:${info.height}` === '1920:1080' || info.width / info.height > 1.7 ? '16:9' : '1:1'];
  const native = useMemo(() => nativeOnlyFeatures(doc), [doc]);
  const duration = tl?.total ?? 0;

  // timeline edit: JSON -> JSON -> canonical text -> patched into the author's text
  const edit = async (fn: (d: ReelDoc) => ReelDoc) => {
    const a = analysisRef.current;
    const src = sourceRef.current;
    if (!a || a.src !== src) {
      toast(diags.some((d) => d.severity === 'error' && d.code.startsWith('E_')) && a?.src !== src ? 'Fix the syntax error first, then edit on the timeline.' : 'One moment: still reading your last change.');
      return;
    }
    const next = fn(a.doc);
    if (next === a.doc) return;
    setOptimistic(next);
    try {
      const [c0, c1] = await Promise.all([canon(a.doc), api.toText(next)]);
      if (sourceRef.current !== src) return setOptimistic(null);
      const r = patchSource(src, c0, c1);
      canonCache.set(next, Promise.resolve(c1));
      setAnalysis({ doc: next, timing: a.timing, src: r.text });
      editor.current?.setText(r.text);
      if (r.normalized) toast('This edit reformatted the script (comments dropped). Undo brings them back.');
    } catch (e) {
      setOptimistic(null);
      toast(e instanceof ApiError ? e.message : 'The edit failed.');
    }
  };

  const fps = tl?.fps ?? 30;
  const sceneOf = (row: T.TLRow) => tl!.scenes[row.sceneIdx];
  const act: InspectorActions = {
    setKey: (k, patch) => edit((d) => T.setKey(d, k, patch, fps)),
    moveKey: (k, t) => edit((d) => T.moveKey(d, k, t, fps)),
    deleteKey: (k) => {
      setSel(null);
      edit((d) => T.deleteKey(d, k));
    },
    setEventExpr: (e, expr) => edit((d) => T.setArgAt(d, e.path, 0, expr.replace(/^@/, '').trim() || e.expr)),
    setProp: (path, key, value) =>
      edit((d) => (key.startsWith('\u0000arg') ? T.setArgAt(d, path, Number(key.slice(4)), String(value ?? '')) : T.setProp(d, path, key, value))),
    setSceneDur: (s, dur) => edit((d) => T.setSceneDur(d, s, dur, fps)),
    addTrack: (row, ch, v) => {
      const tr = row.tracks.find((x) => x.channel === ch);
      edit((d) => (tr ? T.addKey(d, tr, time, v, fps) : T.addTrack(d, sceneOf(row), row.id, ch, time, v, fps)));
    },
    seek: (t) => {
      setPlaying(false);
      setTime(t);
    },
  };

  // keyboard: space, undo/redo outside the editor, delete key, frame steps
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const tgt = e.target as HTMLElement;
      const typing = tgt.closest('.cm-editor, input, textarea, select, [contenteditable]');
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === 'z' && !typing) {
        e.preventDefault();
        e.shiftKey ? editor.current?.redo() : editor.current?.undo();
      } else if (mod && e.key.toLowerCase() === 'y' && !typing) {
        e.preventDefault();
        editor.current?.redo();
      } else if (typing) return;
      else if (e.key === ' ') {
        e.preventDefault();
        setPlaying((p) => !p);
      } else if ((e.key === 'Delete' || e.key === 'Backspace') && sel?.type === 'key') {
        const k = tl?.rows.flatMap((r) => r.tracks.flatMap((t) => t.keys)).find((k) => k.uid === sel.uid);
        if (k && !k.locked) act.deleteKey(k);
      } else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
        setPlaying(false);
        const step = (e.shiftKey ? 1 : 1 / fps) * (e.key === 'ArrowRight' ? 1 : -1);
        setTime((t) => Math.max(0, Math.min(duration, t + step)));
      } else if (e.key === 'Escape') setSel(null);
    };
    addEventListener('keydown', on);
    return () => removeEventListener('keydown', on);
  });

  const vocabRef = useLatest({
    kinds: [...new Set([...(catalog?.elements ?? T.ELEMENT_KINDS), 'scene', 'video', 'anim', 'say', 'set', 'when', 'theme', 'music', 'enter', 'exit', 'pulse', 'move', 'morph', 'camera'])].sort(),
    channels: catalog?.channels ?? ['x', 'y', 'scale', 'rot', 'opacity', 'blur', 'color'],
    eases: easeGroups(catalog).flatMap((g) => g.names),
  });

  const startResize = (e: PointerEvent) => {
    e.preventDefault();
    const y0 = e.clientY, h0 = tlH;
    const mv = (ev: PointerEvent) => setTlH(Math.max(140, Math.min(innerHeight * 0.7, h0 - (ev.clientY - y0))));
    const up = () => {
      removeEventListener('pointermove', mv);
      removeEventListener('pointerup', up);
    };
    addEventListener('pointermove', mv);
    addEventListener('pointerup', up);
  };

  if (initial === null) return <div class="insp-empty" style={{ margin: 'auto' }}>Loading…</div>;

  return (
    <div class="studio" style={{ '--tl-h': `${tlH}px` }} data-testid="studio">
      <section class="pane editor-pane">
        <div class="pane-head">
          <span class="panel-title">Script</span>
          <button class="btn ghost sm icon" onClick={() => editor.current?.undo()} disabled={!hist.undo} title="Undo (Ctrl/⌘ Z)" aria-label="Undo" data-testid="undo">
            <Undo />
          </button>
          <button class="btn ghost sm icon" onClick={() => editor.current?.redo()} disabled={!hist.redo} title="Redo (Shift Ctrl/⌘ Z)" aria-label="Redo" data-testid="redo">
            <Redo />
          </button>
        </div>
        <CodeEditor ref={editor} initial={initial} diagnostics={diags} vocab={() => vocabRef.current} onChange={(t) => setSource(t)} onHistory={setHist} />
        <Diagnostics diags={diags} onJump={(d) => editor.current?.jumpTo(d.line || 1, d.col || 1)} onFix={(d) => editor.current?.applyFix(d)} />
      </section>

      <section class="stage studio-stage">
        <div class="stage-canvas">
          <Preview
            source={analysis?.src ?? (source || null)}
            format={format}
            baseSize={formatSize(info, format || null)}
            time={time}
            playing={playing}
            onTime={setTime}
            exact={exact}
            nativeFeatures={native}
          />
        </div>
        <Transport
          time={time}
          duration={duration}
          fps={fps}
          playing={playing}
          onPlay={setPlaying}
          onSeek={(t) => {
            setPlaying(false);
            setTime(t);
          }}
          right={
            <>
              <button class={'btn sm' + (exact === 'always' ? '' : ' ghost')} title={exact === 'always' ? 'Exact frames always on (native renderer when paused)' : 'Show the exact native frame when paused'} onClick={() => setExact(exact === 'always' ? 'auto' : 'always')} aria-pressed={exact === 'always'} data-testid="exact-toggle">
                <Eye size={14} /> Exact
              </button>
              {formats.length > 1 && (
                <div class="seg" role="group" aria-label="Format">
                  {formats.map((f) => (
                    <button key={f} aria-pressed={(format || formats[0]) === f} onClick={() => setFormat(f === formats[0] ? '' : f)}>
                      {f}
                    </button>
                  ))}
                </div>
              )}
              <div style={{ position: 'relative' }}>
                <button class="btn sm primary" onClick={() => setShowRender(!showRender)} data-testid="studio-render">
                  <Film size={14} /> Render
                </button>
                {showRender && (
                  <div class="popover" style={{ right: 0, bottom: 40 }}>
                    <div class="row between" style={{ marginBottom: 10 }}>
                      <strong>Render video</strong>
                      <button class="btn ghost sm icon" onClick={() => setShowRender(false)} aria-label="Close">
                        <X />
                      </button>
                    </div>
                    <RenderPanel source={source} format={format} fileStem={info.title.toLowerCase().replace(/\W+/g, '-') || 'agentfx'} disabled={diags.some((d) => d.severity === 'error')} />
                  </div>
                )}
              </div>
            </>
          }
        />
      </section>

      <section class="pane inspector-pane">
        <div class="pane-head">
          <span class="panel-title">Inspector</span>
        </div>
        <div class="scroll" style={{ flex: 1, minHeight: 0 }}>
          <Inspector doc={doc} tl={tl} sel={sel} catalog={catalog} time={time} act={act} />
        </div>
      </section>

      <section class="pane timeline-pane">
        <div class="tl-resize" onPointerDown={startResize} />
        {tl ? (
          <Timeline
            tl={tl}
            time={time}
            onSeek={(t) => {
              setPlaying(false);
              setTime(t);
            }}
            sel={sel}
            onSelect={setSel}
            onMoveKey={act.moveKey}
            onMoveEvent={(ev, t) => edit((d) => T.moveEvent(d, ev, t, tl.scenes[ev.sceneIdx].start, fps))}
            onSceneDur={act.setSceneDur}
            onAddKey={(track, _row, t) => {
              const prev = [...track.keys].reverse().find((k) => k.t <= t) ?? track.keys[0];
              edit((d) => T.addKey(d, track, t, prev?.value ?? '0', fps));
            }}
          />
        ) : (
          <div class="insp-empty">Reading the script…</div>
        )}
      </section>
    </div>
  );
}

export { BASE_GROUPS };
