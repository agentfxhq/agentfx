import { useEffect, useState } from 'preact/hooks';
import { safeStorage } from './hooks';

export interface Route {
  view: 'templates' | 'studio';
  template: string | null;
  embed: boolean;
}

export function parseRoute(loc: Location = location): Route {
  const q = new URLSearchParams(loc.search);
  const path = loc.pathname.replace(/\/+$/, '');
  const embedPath = /\/embed(\/([\w-]+))?$/.exec(path);
  return {
    view: /\/studio$/.test(path) ? 'studio' : 'templates',
    template: q.get('t') ?? embedPath?.[2] ?? null,
    embed: q.get('embed') === '1' || !!embedPath,
  };
}

export function useRoute(): Route {
  const [r, setR] = useState(parseRoute());
  useEffect(() => {
    const on = () => setR(parseRoute());
    addEventListener('popstate', on);
    addEventListener('afx:navigate', on);
    return () => {
      removeEventListener('popstate', on);
      removeEventListener('afx:navigate', on);
    };
  }, []);
  return r;
}

export function navigate(url: string, replace = false) {
  if (replace) history.replaceState(null, '', url);
  else history.pushState(null, '', url);
  dispatchEvent(new Event('afx:navigate'));
}

export type Theme = 'dark' | 'light';

export function initialTheme(): Theme {
  const q = new URLSearchParams(location.search).get('theme');
  if (q === 'light' || q === 'dark') return q;
  const s = safeStorage('afx:theme');
  if (s === 'light' || s === 'dark') return s;
  return matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

export function applyTheme(t: Theme) {
  document.documentElement.dataset.theme = t;
}
