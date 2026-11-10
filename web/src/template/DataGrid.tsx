// Editable table for `data` params, with CSV paste (also straight into a cell: multi-line pastes
// become rows).
import { useState } from 'preact/hooks';
import type { DataColumn, DataRow, TemplateParam } from '../api';
import { t } from '../i18n';
import { csvToRows, rowsToCsv } from '../model/csv';
import { Plus, X } from '../ui/icons';

interface Props {
  param: TemplateParam;
  rows: DataRow[];
  onChange: (rows: DataRow[]) => void;
}

export function columnsOf(param: TemplateParam, rows: DataRow[]): DataColumn[] {
  if (param.columns?.length) return param.columns;
  const keys = [...new Set(rows.flatMap((r) => Object.keys(r)))];
  return (keys.length ? keys : ['value']).map((id) => ({ id, label: id, type: rows.every((r) => typeof r[id] === 'number') ? 'number' : 'text' }));
}

export function DataGrid({ param, rows, onChange }: Props) {
  const cols = columnsOf(param, rows);
  const [csv, setCsv] = useState<string | null>(null);
  const max = param.max_rows ?? 200;
  const set = (i: number, k: string, v: string) => {
    const col = cols.find((c) => c.id === k);
    const n = Number(v.replace(',', '.'));
    const next = rows.slice();
    next[i] = { ...next[i], [k]: col?.type === 'number' && v.trim() !== '' && !Number.isNaN(n) ? n : v };
    onChange(next);
  };
  return (
    <div class="datagrid" data-testid={`data-${param.id}`}>
      {csv === null ? (
        <>
          <div class="dg-table" style={{ gridTemplateColumns: `${cols.map((c) => (c.type === 'number' ? '0.7fr' : '1.3fr')).join(' ')} 28px` }}>
            {cols.map((c) => (
              <div key={c.id} class="dg-head">
                {c.label ?? c.id}
              </div>
            ))}
            <div class="dg-head" />
            {rows.map((r, i) => (
              <>
                {cols.map((c) => (
                  <input
                    key={`${i}-${c.id}`}
                    class={'dg-cell' + (c.type === 'number' ? ' num' : '')}
                    value={String(r[c.id] ?? '')}
                    inputMode={c.type === 'number' ? 'decimal' : undefined}
                    aria-label={`${c.label ?? c.id} ${i + 1}`}
                    onInput={(e) => set(i, c.id, (e.target as HTMLInputElement).value)}
                    onPaste={(e) => {
                      const text = e.clipboardData?.getData('text') ?? '';
                      if (!/[\n\t,;]/.test(text.trim())) return;
                      e.preventDefault();
                      const pasted = csvToRows(text, cols);
                      onChange([...rows.slice(0, i), ...pasted, ...rows.slice(i + 1)].slice(0, max));
                    }}
                  />
                ))}
                <button type="button" class="dg-del" aria-label={t('removeRow')} title={t('removeRow')} onClick={() => onChange(rows.filter((_, j) => j !== i))}>
                  <X size={12} />
                </button>
              </>
            ))}
          </div>
          <div class="row" style={{ marginTop: 8 }}>
            <button type="button" class="btn sm" disabled={rows.length >= max} onClick={() => onChange([...rows, Object.fromEntries(cols.map((c) => [c.id, c.type === 'number' ? 0 : '']))])} data-testid="add-row">
              <Plus size={13} /> {t('addRow')}
            </button>
            <button type="button" class="btn sm ghost" onClick={() => setCsv(rowsToCsv(rows, cols))} data-testid="paste-csv">
              {t('pasteCsv')}
            </button>
          </div>
        </>
      ) : (
        <>
          <textarea class="textarea mono" rows={6} value={csv} onInput={(e) => setCsv((e.target as HTMLTextAreaElement).value)} spellcheck={false} aria-label="CSV" data-testid="csv-text" />
          <div class="faint" style={{ fontSize: 12, margin: '6px 0 8px' }}>{t('csvHint')}</div>
          <div class="row">
            <button type="button" class="btn sm primary" onClick={() => {
              onChange(csvToRows(csv, cols).slice(0, max));
              setCsv(null);
            }} data-testid="apply-csv">
              {t('apply')}
            </button>
            <button type="button" class="btn sm ghost" onClick={() => setCsv(null)}>
              {t('cancel')}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
