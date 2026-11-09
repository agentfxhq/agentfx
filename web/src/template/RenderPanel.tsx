// Render -> progress -> download (POST /renders, GET /renders/{id}); several files (sizes) come
// back as a list plus a zip.
import { useEffect, useRef, useState } from 'preact/hooks';
import { absUrl, api, ApiError, type Output, type RenderStatus } from '../api';
import { prettySize, t } from '../i18n';
import { Check, Download, Film, Image as ImageIcon } from '../ui/icons';

type Done = { s: 'done'; url: string; output: Output; files: { name: string; size?: string | null; url: string }[]; zip?: string };
type State = { s: 'idle' } | { s: 'running'; progress: number } | Done | { s: 'error'; message: string };

interface Props {
  source: string | null;
  format: string;
  fileStem: string;
  disabled?: boolean;
  compact?: boolean;
  still?: boolean;
  sizes?: Record<string, string>; // name -> WxH (stills / size picker)
  at?: string;
}

/** Poll a render or batch job until it ends. */
export function pollJob(job: string, onUpdate: (r: RenderStatus) => void, timer: { current?: ReturnType<typeof setTimeout> }) {
  const tick = async () => {
    try {
      const r = await api.renderStatus(job);
      onUpdate(r);
      if (r.status === 'queued' || r.status === 'running') timer.current = setTimeout(tick, 700);
    } catch (e) {
      onUpdate({ status: 'error', progress: 0, error: e instanceof ApiError ? e.message : t('lostJob') });
    }
  };
  tick();
}

export function doneState(r: RenderStatus, output: Output): Done {
  const files = (r.files ?? (r.url ? [{ name: `agentfx.${output}`, url: r.url }] : [])).map((f) => ({ ...f, url: absUrl(f.url) }));
  return { s: 'done', url: absUrl(r.url ?? files[0]?.url ?? ''), output, files, zip: r.zip_url ? absUrl(r.zip_url) : undefined };
}

export function RenderPanel({ source, format, fileStem, disabled, compact, still, sizes, at }: Props) {
  const outputs: Output[] = still ? ['png', 'jpg', 'pdf'] : ['mp4', 'gif', 'webp'];
  const [output, setOutput] = useState<Output>(outputs[0]);
  const [st, setSt] = useState<State>({ s: 'idle' });
  const poll = useRef<ReturnType<typeof setTimeout>>();
  const renderedFor = useRef<string | null>(null);
  const key = `${source}|${format}|${JSON.stringify(sizes ?? {})}|${output}`;

  useEffect(() => () => clearTimeout(poll.current), []);
  useEffect(() => {
    if (!outputs.includes(output)) setOutput(outputs[0]);
  }, [still]);
  // an edit after a finished render makes the result stale
  useEffect(() => {
    if (st.s === 'done' && renderedFor.current !== key) setSt({ s: 'idle' });
  }, [key]);

  const noSize = !!sizes && Object.keys(sizes).length === 0;
  const start = async () => {
    if (!source || noSize) return;
    clearTimeout(poll.current);
    setSt({ s: 'running', progress: 0 });
    try {
      const { job_id } = await api.startRender(source, sizes ? undefined : format || undefined, output, { sizes, at });
      renderedFor.current = key;
      pollJob(job_id, (r) => {
        if (r.status === 'done') setSt(doneState(r, output));
        else if (r.status === 'error') setSt({ s: 'error', message: r.error ?? t('renderFailed') });
        else setSt({ s: 'running', progress: r.progress ?? 0 });
      }, poll);
    } catch (e) {
      setSt({ s: 'error', message: e instanceof ApiError ? friendly(e) : t('startFailed') });
    }
  };

  const running = st.s === 'running';
  const isImg = (o: Output) => o === 'png' || o === 'jpg' || o === 'gif' || o === 'webp';
  return (
    <div class="render-panel" data-testid="render-panel" style={{ display: 'grid', gap: 12 }}>
      {!compact && (
        <div class="row between">
          <span class="panel-title">{t('export')}</span>
          <div class="seg" role="group" aria-label={t('fileType')}>
            {outputs.map((o) => (
              <button key={o} type="button" aria-pressed={output === o} onClick={() => setOutput(o)} disabled={running} data-output={o}>
                {o.toUpperCase()}
              </button>
            ))}
          </div>
        </div>
      )}
      {st.s === 'done' ? (
        <div class="render-done" data-testid="render-done">
          {st.files.length <= 1 ? (
            st.output === 'mp4' ? <video src={st.url} autoplay muted loop playsInline /> : isImg(st.output) ? <img src={st.url} alt="" /> : null
          ) : (
            <ul class="file-list" data-testid="file-list">
              {st.files.map((f) => (
                <li key={f.url}>
                  <span class="ico">{isImg(st.output) ? <ImageIcon size={14} /> : <Film size={14} />}</span>
                  <span class="nm">{prettySize(f.name.replace(/^agentfx-|\.\w+$/g, ''))}</span>
                  <span class="faint mono">{f.size}</span>
                  <a class="btn sm ghost icon" href={f.url} download={f.name} aria-label={t('download', { t: f.name })} title={f.name}>
                    <Download size={14} />
                  </a>
                </li>
              ))}
            </ul>
          )}
          <div class="row">
            <a class="btn primary" style={{ flex: 1 }} href={st.zip ?? st.url} download={st.zip ? `${fileStem}.zip` : st.files[0]?.name ?? `${fileStem}.${st.output}`} data-testid="download">
              <Download /> {st.zip ? t('downloadAll') : t('download', { t: st.output.toUpperCase() })}
            </a>
            <button class="btn" onClick={start} title={t('renderAgain')}>
              {t('again')}
            </button>
          </div>
        </div>
      ) : (
        <button class="btn primary lg block" onClick={start} disabled={disabled || running || !source || noSize} data-testid="render">
          {running ? (
            <>
              <span class="preview-loading on" style={{ position: 'static', borderColor: 'rgba(0,0,0,.2)', borderTopColor: 'currentColor' }} />
              {t('rendering', { p: Math.round((st as { progress: number }).progress * 100) })}
            </>
          ) : (
            <>
              {still ? <ImageIcon /> : <Film />} {still ? t('renderStill') : t('render')}
            </>
          )}
        </button>
      )}
      {running && (
        <div class="progress" aria-label="Render progress" data-testid="progress">
          <div style={{ width: `${Math.max(4, (st as { progress: number }).progress * 100)}%` }} />
        </div>
      )}
      {st.s === 'error' && <div class="err-text">{st.message}</div>}
      {noSize && <div class="err-text">{t('pickSize')}</div>}
      {st.s === 'idle' && !compact && !noSize && (
        <div class="faint" style={{ fontSize: 12, display: 'flex', gap: 6, alignItems: 'center' }}>
          <Check size={13} /> {still ? t('freeStill') : t('freeTier')}
        </div>
      )}
    </div>
  );
}

function friendly(e: ApiError): string {
  if (e.status === 429) return t('limit');
  if (e.status === 413) return t('tooLong');
  return e.message;
}
