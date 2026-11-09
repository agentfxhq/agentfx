// CSV <-> rows for `data` params and batch input (RFC 4180 quoting; comma, semicolon or tab).
import type { DataColumn, DataRow } from '../api';

export function parseCsv(text: string): string[][] {
  const t = text.replace(/\r\n?/g, '\n').replace(/\n+$/, '');
  if (!t.trim()) return [];
  const first = t.split('\n')[0];
  const sep = [',', ';', '\t'].map((c) => [c, first.split(c).length] as const).sort((a, b) => b[1] - a[1])[0][0];
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = '';
  let q = false;
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (q) {
      if (c === '"' && t[i + 1] === '"') {
        cell += '"';
        i++;
      } else if (c === '"') q = false;
      else cell += c;
    } else if (c === '"' && cell === '') q = true;
    else if (c === sep) {
      row.push(cell);
      cell = '';
    } else if (c === '\n') {
      row.push(cell);
      rows.push(row);
      row = [];
      cell = '';
    } else cell += c;
  }
  row.push(cell);
  rows.push(row);
  return rows.map((r) => r.map((c) => c.trim())).filter((r) => r.some((c) => c));
}

/** Rows as objects; the first line is a header when it names any known column. */
export function csvToRows(text: string, columns: DataColumn[]): DataRow[] {
  const cells = parseCsv(text);
  if (!cells.length) return [];
  const ids = columns.map((c) => c.id);
  const labels = columns.map((c) => (c.label ?? c.id).toLowerCase());
  const head = cells[0].map((h) => h.toLowerCase());
  const hasHeader = head.some((h) => ids.includes(h) || labels.includes(h));
  const keys = hasHeader ? head.map((h) => ids[labels.indexOf(h)] ?? (ids.includes(h) ? h : h)) : ids;
  return (hasHeader ? cells.slice(1) : cells).map((r) => {
    const o: DataRow = {};
    keys.forEach((k, i) => {
      if (k === undefined || r[i] === undefined) return;
      const col = columns.find((c) => c.id === k);
      const n = Number(r[i].replace(',', '.'));
      o[k] = col?.type === 'number' && r[i] !== '' && !Number.isNaN(n) ? n : r[i];
    });
    return o;
  });
}

export function rowsToCsv(rows: DataRow[], columns: DataColumn[]): string {
  const esc = (v: unknown) => {
    const s = String(v ?? '');
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  };
  return [columns.map((c) => c.id).join(','), ...rows.map((r) => columns.map((c) => esc(r[c.id])).join(','))].join('\n');
}
