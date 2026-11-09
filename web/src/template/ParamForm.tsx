// Form generated from the template manifest's params (docs/PLATFORM.md "Templates").
import { useRef, useState } from 'preact/hooks';
import { api, ApiError, type DataRow, type TemplateParam } from '../api';
import { t } from '../i18n';
import { DataGrid } from './DataGrid';
import { Image as ImageIcon, Music, X } from '../ui/icons';

export type ParamValue = string | number | DataRow[];
export type ParamValues = Record<string, ParamValue>;
export interface AssetLabel {
  [paramId: string]: string; // file name shown for uploaded assets
}

export function defaults(params: TemplateParam[]): ParamValues {
  const v: ParamValues = {};
  for (const p of params) if (p.default !== undefined && p.type !== 'audio' && p.type !== 'image') v[p.id] = p.default as ParamValue;
  return v;
}

const COLOR_RE = /^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$/;
const PALETTE = ['#3CBF87', '#75F6D4', '#5EEAD4', '#60A5FA', '#A78BFA', '#FF48B0', '#E8442E', '#F2C14E', '#FFFFFF', '#10121D'];

interface Props {
  params: TemplateParam[];
  values: ParamValues;
  assets: AssetLabel;
  onChange: (id: string, v: ParamValue | undefined, assetName?: string) => void;
  optionLabel?: (pid: string, o: string) => string | null;
}

export function ParamForm({ params, values, assets, onChange, optionLabel }: Props) {
  return (
    <div data-testid="param-form">
      {params.map((p) => (
        <div class="field" key={p.id}>
          <Field p={p} value={values[p.id]} assetName={assets[p.id]} onChange={(v, name) => onChange(p.id, v, name)} optionLabel={(o) => optionLabel?.(p.id, o) ?? null} />
        </div>
      ))}
    </div>
  );
}

function Field({ p, value, assetName, onChange, optionLabel }: { p: TemplateParam; value: ParamValue | undefined; assetName?: string; onChange: (v: ParamValue | undefined, name?: string) => void; optionLabel: (o: string) => string | null }) {
  const id = `param-${p.id}`;
  const label = (extra?: preact.ComponentChildren) => (
    <label class="field-label" for={id}>
      <span>{p.label}</span>
      {extra}
    </label>
  );
  switch (p.type) {
    case 'text': {
      const v = String(value ?? '');
      return (
        <>
          {label(p.max_len ? <span class="count">{v.length}/{p.max_len}</span> : null)}
          <input id={id} class="input" value={v} maxLength={p.max_len} onInput={(e) => onChange((e.target as HTMLInputElement).value.replace(/[\r\n]/g, ' '))} data-param={p.id} />
        </>
      );
    }
    case 'longtext': {
      const v = String(value ?? '');
      return (
        <>
          {label(p.max_len ? <span class="count">{t('lines', { n: v.split('\n').filter((l) => l.trim()).length })}</span> : null)}
          <textarea id={id} class="textarea" value={v} maxLength={p.max_len} rows={5} onInput={(e) => onChange((e.target as HTMLTextAreaElement).value)} data-param={p.id} />
        </>
      );
    }
    case 'color':
      return <ColorField p={p} id={id} value={String(value ?? p.default ?? '#000000')} onChange={onChange} label={label()} />;
    case 'number': {
      const min = p.min ?? 0, max = p.max ?? 100, step = p.step ?? (max - min) / 100;
      const v = Number(value ?? p.default ?? min);
      const pct = ((v - min) / (max - min || 1)) * 100;
      return (
        <>
          {label()}
          <div class="slider">
            <input id={id} type="range" min={min} max={max} step={step} value={v} style={{ '--p': `${pct}%` }} onInput={(e) => onChange(Number((e.target as HTMLInputElement).value))} data-param={p.id} />
            <input class="input sm mono" type="number" min={min} max={max} step={step} value={v} aria-label={p.label} onChange={(e) => {
              const n = Number((e.target as HTMLInputElement).value);
              if (!Number.isNaN(n)) onChange(Math.min(max, Math.max(min, n)));
            }} />
          </div>
        </>
      );
    }
    case 'enum': {
      const opts = p.options ?? [];
      const short = opts.length <= 4 && opts.every((o) => o.length <= 12);
      return (
        <>
          {label()}
          {short ? (
            <div class="seg wide" role="group" aria-label={p.label}>
              {opts.map((o) => (
                <button type="button" key={o} aria-pressed={value === o} onClick={() => onChange(o)}>
                  {optionLabel(o) ?? pretty(o)}
                </button>
              ))}
            </div>
          ) : (
            <select id={id} class="select" value={String(value ?? '')} onChange={(e) => onChange((e.target as HTMLSelectElement).value)} data-param={p.id}>
              {opts.map((o) => (
                <option key={o} value={o}>
                  {optionLabel(o) ?? pretty(o)}
                </option>
              ))}
            </select>
          )}
        </>
      );
    }
    case 'audio':
    case 'image':
      return <AssetField p={p} id={id} name={assetName} hasValue={value !== undefined} onChange={onChange} label={label()} />;
    case 'data': {
      const rows = Array.isArray(value) ? value : [];
      return (
        <>
          {label(<span class="count">{t('rows', { n: rows.length })}</span>)}
          <DataGrid param={p} rows={rows} onChange={onChange} />
        </>
      );
    }
    default:
      return null;
  }
}

function pretty(o: string): string {
  if (/[.(]/.test(o)) return o;
  return o.replace(/[-_]/g, ' ').replace(/^\w/, (c) => c.toUpperCase());
}

function ColorField({ p, id, value, onChange, label }: { p: TemplateParam; id: string; value: string; onChange: (v: ParamValue) => void; label: preact.ComponentChildren }) {
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? value;
  const valid = COLOR_RE.test(shown);
  return (
    <>
      {label}
      <div class="color-field">
        <label class="swatch" style={{ background: COLOR_RE.test(value) ? value.slice(0, 7) : '#000' }} title={t('pickColour')}>
          <input type="color" value={value.slice(0, 7)} onInput={(e) => onChange((e.target as HTMLInputElement).value.toUpperCase())} aria-label={`${p.label} picker`} />
        </label>
        <input
          id={id}
          class="input mono"
          value={shown}
          spellcheck={false}
          style={valid ? undefined : { borderColor: 'var(--danger)' }}
          onInput={(e) => {
            let v = (e.target as HTMLInputElement).value.trim();
            if (v && !v.startsWith('#')) v = '#' + v;
            setDraft(v);
            if (COLOR_RE.test(v)) onChange(v.toUpperCase());
          }}
          onBlur={() => setDraft(null)}
          data-param={p.id}
        />
      </div>
      <div class="swatches">
        {PALETTE.map((c) => (
          <button type="button" key={c} style={{ background: c }} title={c} aria-label={`Use ${c}`} onClick={() => onChange(c)} />
        ))}
      </div>
    </>
  );
}

function AssetField({ p, id, name, hasValue, onChange, label }: { p: TemplateParam; id: string; name?: string; hasValue: boolean; onChange: (v: ParamValue | undefined, name?: string) => void; label: preact.ComponentChildren }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const accept = p.type === 'audio' ? 'audio/mpeg,audio/wav,audio/x-wav,audio/ogg,audio/mp4,.mp3,.wav,.ogg,.m4a' : 'image/png,image/jpeg,image/webp,image/svg+xml';
  const upload = async (f: File | undefined) => {
    if (!f) return;
    setBusy(true);
    setErr(null);
    try {
      const r = await api.upload(f);
      onChange(r.asset_id, f.name);
    } catch (e) {
      setErr(e instanceof ApiError ? e.message : t('uploadFailed'));
    } finally {
      setBusy(false);
    }
  };
  const Icon = p.type === 'audio' ? Music : ImageIcon;
  return (
    <>
      {label}
      <input ref={input} id={id} type="file" accept={accept} hidden onChange={(e) => upload((e.target as HTMLInputElement).files?.[0])} data-param={p.id} />
      <div class="row">
        <button
          type="button"
          class={'drop' + (over ? ' over' : '')}
          onClick={() => input.current?.click()}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setOver(false);
            upload(e.dataTransfer?.files?.[0]);
          }}
        >
          <span class="ico">
            <Icon />
          </span>
          <span style={{ minWidth: 0 }}>
            <strong style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{busy ? t('uploading') : hasValue && name ? name : p.type === 'audio' ? t('dropTrack') : t('dropImage')}</strong>
            {hasValue && name ? t('replace') : p.default ? t('usingDemo') : t('browse')}
          </span>
        </button>
        {hasValue && (
          <button type="button" class="btn ghost icon" aria-label={t('remove')} onClick={() => onChange(undefined)}>
            <X />
          </button>
        )}
      </div>
      {err && <div class="err-text">{err}</div>}
    </>
  );
}
