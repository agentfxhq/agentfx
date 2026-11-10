// Client for the AgentFX HTTP API (docs/PLATFORM.md "HTTP API").
import type { ReelDoc } from './model/reel';
import type { Timing } from './model/timeline';

export const API_BASE: string = (import.meta.env.VITE_API_BASE as string | undefined) ?? '/api/v1';

export interface Diagnostic {
  code: string;
  severity: 'error' | 'warning' | 'info';
  message: string;
  hint?: string;
  line: number;
  col: number;
  id?: string;
  at?: number;
  format?: string;
}

export type ParamType = 'text' | 'longtext' | 'color' | 'number' | 'enum' | 'audio' | 'image' | 'data';

export interface DataColumn {
  id: string;
  label?: string;
  type?: 'text' | 'number';
}
export type DataRow = Record<string, string | number>;

export interface TemplateParam {
  id: string;
  type: ParamType;
  label: string;
  default?: string | number | DataRow[];
  max_len?: number;
  min?: number;
  max?: number;
  step?: number;
  options?: string[];
  required?: boolean;
  columns?: DataColumn[];
  max_rows?: number;
}

export interface TemplateManifest {
  id: string;
  title: string;
  description: string;
  tags: string[];
  formats: string[];
  duration_s: number;
  kind?: 'video' | 'still';
  still_at?: string;
  /** size preset names (templates/render_template.py SIZE_PRESETS); the first is the main size */
  sizes?: string[];
  /** name -> "WxH" for `sizes` (service addition) */
  size_px?: Record<string, string>;
  i18n?: Record<string, { title?: string; description?: string; params?: Record<string, string>; options?: Record<string, Record<string, string>> }>;
  params: TemplateParam[];
  preview_url?: string;
  source?: string;
}

export interface RenderStatus {
  status: 'queued' | 'running' | 'done' | 'error';
  progress: number;
  url?: string;
  files?: { name: string; size?: string | null; row?: number; url: string }[];
  error?: string;
}

export type Output = 'mp4' | 'gif' | 'webp' | 'png' | 'jpg' | 'pdf';

export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string, public diagnostics: Diagnostic[] = []) {
    super(message);
  }
}

async function req(path: string, init: RequestInit & { json?: unknown } = {}): Promise<Response> {
  const { json, ...rest } = init;
  const headers: Record<string, string> = { ...(rest.headers as Record<string, string>) };
  if (json !== undefined) headers['Content-Type'] = 'application/json';
  const key = typeof localStorage !== 'undefined' ? safeGet('afx:apiKey') : null;
  if (key) headers.Authorization = `Bearer ${key}`;
  let res: Response;
  try {
    res = await fetch(API_BASE + path, { ...rest, headers, body: json !== undefined ? JSON.stringify(json) : rest.body });
  } catch (e) {
    if ((e as Error).name === 'AbortError') throw e;
    throw new ApiError(0, 'network', 'The render service is not reachable.');
  }
  if (!res.ok) {
    let err: { code?: string; message?: string; diagnostics?: Diagnostic[] } = {};
    try {
      err = (await res.json()).error ?? {};
    } catch {
      /* not JSON */
    }
    throw new ApiError(res.status, err.code ?? 'http_' + res.status, err.message ?? res.statusText, err.diagnostics ?? []);
  }
  return res;
}

function safeGet(k: string): string | null {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
}

const j = async <T>(p: Promise<Response>): Promise<T> => (await p).json() as Promise<T>;

export const api = {
  templates: (signal?: AbortSignal) => j<TemplateManifest[]>(req('/templates', { signal })),
  template: (id: string, signal?: AbortSignal) => j<TemplateManifest>(req(`/templates/${encodeURIComponent(id)}`, { signal })),
  /** `size`: a preset name of a still template (the source is laid out for that size). */
  instantiate: (template: string, params: Record<string, unknown>, signal?: AbortSignal, size?: string) =>
    j<{ source: string }>(req('/instantiate', { method: 'POST', json: { template, params, size }, signal })).then((r) => r.source),
  toJson: (source: string, signal?: AbortSignal) =>
    j<{ json: ReelDoc }>(req('/parse', { method: 'POST', json: { source, to: 'json' }, signal })).then((r) => r.json),
  toText: (doc: ReelDoc, signal?: AbortSignal) =>
    j<{ source: string }>(req('/parse', { method: 'POST', json: { json: doc, to: 'text' }, signal })).then((r) => r.source),
  lint: (source: string, format?: string, signal?: AbortSignal) =>
    j<{ diagnostics: Diagnostic[] }>(req('/lint', { method: 'POST', json: { source, format }, signal })).then((r) => r.diagnostics),
  timing: (source: string, format?: string, signal?: AbortSignal) => j<Timing>(req('/timing', { method: 'POST', json: { source, format }, signal })),
  frame: async (source: string, at: number, format?: string, width?: number, signal?: AbortSignal, size?: string) => {
    const r = await req('/frame', { method: 'POST', json: { source, at: `${at.toFixed(3)}s`, format, width, size }, signal });
    return URL.createObjectURL(await r.blob());
  },
  /** API addition (PLATFORM.md): the HTML backend of the script for the live preview. */
  preview: async (source: string, format?: string, signal?: AbortSignal, size?: string) =>
    (await req('/preview', { method: 'POST', json: { source, format, size }, signal })).text(),
  /** API addition (PLATFORM.md): the capability catalog (`agentfx catalog`). */
  catalog: (signal?: AbortSignal) => j<Catalog>(req('/catalog', { signal })),
  upload: async (file: File) => {
    const fd = new FormData();
    fd.append('file', file);
    return j<{ asset_id: string; name: string }>(req('/assets', { method: 'POST', body: fd }));
  },
  /** A raw script (Studio): one file; stills render the frame at `at`. */
  startRender: (source: string, format: string | undefined, output: Output, opts: { quality?: 'preview' | 'hd'; at?: string } = {}) =>
    j<{ job_id: string }>(req('/renders', { method: 'POST', json: { source, format, output, quality: opts.quality ?? 'preview', at: opts.at } })),
  /** A template: the service instantiates it per size (preset names), one file per size. */
  renderTemplate: (template: string, params: Record<string, unknown>, output: Output, opts: { format?: string; sizes?: string[]; quality?: 'preview' | 'hd' } = {}) =>
    j<{ job_id: string }>(req('/renders', { method: 'POST', json: { template, params, output, format: opts.format, sizes: opts.sizes, quality: opts.quality ?? 'preview' } })),
  /** One output per row of params (x sizes); the finished job's `url` is the zip. */
  batch: (template: string, rows: Record<string, unknown>[], output: Output, sizes?: string[], params?: Record<string, unknown>) =>
    j<{ job_id: string }>(req('/batch', { method: 'POST', json: { template, rows, output, sizes, params } })),
  renderStatus: (id: string, signal?: AbortSignal) => j<RenderStatus>(req(`/renders/${encodeURIComponent(id)}`, { signal })),
};

export interface Catalog {
  features: { name: string; level: string; group: string; syntax: string; doc: string; status: string }[];
  channels: string[];
  elements: string[];
}

export function absUrl(u: string): string {
  if (/^https?:/.test(u)) return u;
  if (u.startsWith('/')) {
    const base = API_BASE.startsWith('http') ? new URL(API_BASE).origin : '';
    return base + u;
  }
  return u;
}
