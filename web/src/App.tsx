import { useEffect, useState } from 'preact/hooks';
import { Studio } from './studio/Studio';
import { TemplateMode } from './template/TemplateMode';
import { safeStorage, useToast } from './ui/hooks';
import { Code, Grid, Moon, Sun } from './ui/icons';
import { applyTheme, initialTheme, navigate, useRoute, type Theme } from './ui/router';

export function App() {
  const route = useRoute();
  const [theme, setTheme] = useState<Theme>(initialTheme());
  const msg = useToast();
  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => {
    document.title = route.view === 'studio' ? 'Studio · AgentFX' : 'AgentFX Playground: animated text, glitch and music videos';
  }, [route.view]);

  const toggleTheme = () => {
    const t = theme === 'dark' ? 'light' : 'dark';
    setTheme(t);
    safeStorage('afx:theme', t);
  };

  return (
    <div class={'app' + (route.embed ? ' embed' : '')}>
      {!route.embed && (
        <header class="header">
          <a class="brand" href="/" onClick={(e) => (e.preventDefault(), navigate('/'))}>
            <span class="logo" aria-hidden="true" />
            AgentFX <small>Playground</small>
          </a>
          <div class="header-center">
            <div class="seg" role="tablist" aria-label="Mode">
              <button role="tab" aria-pressed={route.view === 'templates'} onClick={() => navigate(`/play${route.template ? `?t=${route.template}` : ''}`)} data-testid="mode-templates">
                <Grid size={14} /> Templates
              </button>
              <button role="tab" aria-pressed={route.view === 'studio'} onClick={() => navigate(`/studio${route.template ? `?t=${route.template}` : ''}`)} data-testid="mode-studio">
                <Code size={14} /> Studio
              </button>
            </div>
          </div>
          <div class="spacer" />
          <a class="btn ghost sm" href="https://agentfx.dev" target="_blank" rel="noopener">
            Docs
          </a>
          <button class="btn ghost sm icon" onClick={toggleTheme} aria-label="Toggle theme" title="Light / dark" data-testid="theme">
            {theme === 'dark' ? <Sun /> : <Moon />}
          </button>
        </header>
      )}
      {route.view === 'studio' ? <Studio key="studio" templateId={route.template} /> : <TemplateMode key="tm" initialId={route.template} embed={route.embed} />}
      {msg && <div class="toast" role="status">{msg}</div>}
    </div>
  );
}
