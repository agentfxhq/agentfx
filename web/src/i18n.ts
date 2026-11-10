// UI strings of template mode (embedded on landing pages, English and German). Language:
// `?lang=de|en`, else the browser language. Template texts come from the manifest's `i18n` block.
import type { TemplateManifest, TemplateParam } from './api';

const en = {
  templates: 'Templates',
  search: 'Glitch, lyrics, music…',
  noMatch: 'No template matches “{q}”.',
  loadFailed: 'Could not load templates: {e}',
  retry: 'Try again',
  export: 'Export',
  fileType: 'File type',
  render: 'Render video',
  renderStill: 'Create images',
  rendering: 'Rendering… {p}%',
  download: 'Download {t}',
  downloadAll: 'Download all (.zip)',
  again: 'Again',
  renderAgain: 'Render again',
  freeTier: 'Free · up to 20 s · 720p with a small watermark',
  freeStill: 'Free · PNG, JPG or print-ready PDF',
  limit: 'Render limit reached for today. Try again tomorrow or use an API key.',
  tooLong: 'This video is too long for the free tier.',
  renderFailed: 'The render failed.',
  lostJob: 'Lost the render job.',
  startFailed: 'Could not start the render.',
  openStudio: 'Open in Studio',
  madeWith: 'Made with',
  format: 'Format',
  sizes: 'Sizes',
  sizesHint: 'Pick one or more; each becomes a file.',
  pickSize: 'Pick at least one size.',
  landscape: 'Landscape',
  vertical: 'Vertical',
  square: 'Square',
  portrait: 'Portrait',
  play: 'Play',
  pause: 'Pause',
  dropTrack: 'Drop your track',
  dropImage: 'Drop an image',
  browse: 'or click to browse',
  usingDemo: 'Using the demo track · mp3 or wav',
  replace: 'Click to replace',
  uploading: 'Uploading…',
  uploadFailed: 'Upload failed',
  remove: 'Remove file',
  pickColour: 'Pick a colour',
  lines: '{n} lines',
  addRow: 'Add row',
  pasteCsv: 'Paste CSV',
  csvHint: 'One row per line, comma separated. A header row with the column names is optional.',
  apply: 'Apply',
  cancel: 'Cancel',
  removeRow: 'Remove row',
  rows: '{n} rows',
  batch: 'Batch from a spreadsheet',
  batchHint: 'Paste CSV with one column per field ({cols}). Every row becomes its own file, delivered as a .zip.',
  batchGo: 'Create {n} files',
  liveApprox: 'Live preview',
  exact: 'Exact frame',
  renderingFrame: 'Rendering frame…',
};

type Dict = typeof en;

const de: Dict = {
  templates: 'Vorlagen',
  search: 'Glitch, Lyrics, Musik…',
  noMatch: 'Keine Vorlage passt zu „{q}“.',
  loadFailed: 'Vorlagen konnten nicht geladen werden: {e}',
  retry: 'Nochmal versuchen',
  export: 'Export',
  fileType: 'Dateityp',
  render: 'Video erstellen',
  renderStill: 'Bilder erstellen',
  rendering: 'Wird erstellt… {p} %',
  download: '{t} herunterladen',
  downloadAll: 'Alle herunterladen (.zip)',
  again: 'Nochmal',
  renderAgain: 'Neu erstellen',
  freeTier: 'Kostenlos · bis 20 s · 720p mit kleinem Wasserzeichen',
  freeStill: 'Kostenlos · PNG, JPG oder druckfertiges PDF',
  limit: 'Tageslimit erreicht. Morgen wieder oder mit API-Schlüssel.',
  tooLong: 'Dieses Video ist für die kostenlose Stufe zu lang.',
  renderFailed: 'Das Erstellen ist fehlgeschlagen.',
  lostJob: 'Der Auftrag ging verloren.',
  startFailed: 'Konnte nicht gestartet werden.',
  openStudio: 'Im Studio öffnen',
  madeWith: 'Gemacht mit',
  format: 'Format',
  sizes: 'Größen',
  sizesHint: 'Eine oder mehrere wählen; jede wird eine Datei.',
  pickSize: 'Mindestens eine Größe wählen.',
  landscape: 'Querformat',
  vertical: 'Hochformat',
  square: 'Quadrat',
  portrait: 'Porträt',
  play: 'Abspielen',
  pause: 'Pause',
  dropTrack: 'Musik hier ablegen',
  dropImage: 'Bild hier ablegen',
  browse: 'oder klicken zum Auswählen',
  usingDemo: 'Demo-Track aktiv · mp3 oder wav',
  replace: 'Klicken zum Ersetzen',
  uploading: 'Wird hochgeladen…',
  uploadFailed: 'Hochladen fehlgeschlagen',
  remove: 'Datei entfernen',
  pickColour: 'Farbe wählen',
  lines: '{n} Zeilen',
  addRow: 'Zeile hinzufügen',
  pasteCsv: 'CSV einfügen',
  csvHint: 'Eine Zeile pro Eintrag, durch Komma getrennt. Eine Kopfzeile mit Spaltennamen ist optional.',
  apply: 'Übernehmen',
  cancel: 'Abbrechen',
  removeRow: 'Zeile entfernen',
  rows: '{n} Zeilen',
  batch: 'Serie aus einer Tabelle',
  batchHint: 'CSV mit einer Spalte pro Feld einfügen ({cols}). Jede Zeile wird eine eigene Datei, geliefert als .zip.',
  batchGo: '{n} Dateien erstellen',
  liveApprox: 'Live-Vorschau',
  exact: 'Exaktes Bild',
  renderingFrame: 'Bild wird berechnet…',
};

const DICTS: Record<string, Dict> = { en, de };

export function detectLang(): string {
  const q = typeof location !== 'undefined' ? new URLSearchParams(location.search).get('lang') : null;
  if (q && DICTS[q]) return q;
  const nav = typeof navigator !== 'undefined' ? navigator.language?.slice(0, 2) : 'en';
  return DICTS[nav] ? nav : 'en';
}

export let lang = detectLang();
export function setLang(l: string) {
  if (DICTS[l]) lang = l;
}

export function t(key: keyof Dict, vars: Record<string, string | number> = {}): string {
  const s = (DICTS[lang] ?? en)[key] ?? en[key];
  return s.replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? ''));
}

/** Title, description and param labels in the current language (manifest `i18n`). */
export function localize(m: TemplateManifest): TemplateManifest {
  const tr = m.i18n?.[lang];
  if (!tr) return m;
  return {
    ...m,
    title: tr.title ?? m.title,
    description: tr.description ?? m.description,
    params: m.params.map((p) => localizeParam(p, tr.params?.[p.id])),
  };
}

function localizeParam(p: TemplateParam, label?: string): TemplateParam {
  return label ? { ...p, label } : p;
}

export function optionLabel(m: TemplateManifest | null, pid: string, o: string): string | null {
  return m?.i18n?.[lang]?.options?.[pid]?.[o] ?? null;
}

export function prettySize(name: string): string {
  return name.replace(/[-_]/g, ' ').replace(/\b(\w)/g, (c) => c.toUpperCase()).replace(/\bA(\d)\b/, 'A$1');
}
