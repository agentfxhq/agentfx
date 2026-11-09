import { useState } from 'preact/hooks';
import type { Diagnostic } from '../api';
import { Check, Chevron, Wand } from '../ui/icons';
import { suggestion } from './CodeEditor';

function md(s: string) {
  return s.split(/(`[^`]+`)/).map((p, i) => (p.startsWith('`') ? <code key={i}>{p.slice(1, -1)}</code> : p));
}

export function Diagnostics({ diags, onJump, onFix }: { diags: Diagnostic[]; onJump: (d: Diagnostic) => void; onFix: (d: Diagnostic) => void }) {
  const [open, setOpen] = useState(true);
  const errors = diags.filter((d) => d.severity === 'error').length;
  const warns = diags.filter((d) => d.severity === 'warning').length;
  return (
    <div class="diag-list" data-testid="diagnostics" style={{ display: 'flex', flexDirection: 'column' }}>
      <div class="diag-head" onClick={() => setOpen(!open)}>
        <Chevron size={12} style={{ transform: open ? 'rotate(90deg)' : '', transition: 'transform .15s', color: 'var(--text-3)' }} />
        <span class="panel-title" style={{ marginRight: 'auto' }}>Problems</span>
        {diags.length === 0 ? (
          <span class="chip accent">
            <Check size={12} /> All clear
          </span>
        ) : (
          <>
            {errors > 0 && <span class="chip danger" data-testid="error-count">{errors} error{errors > 1 ? 's' : ''}</span>}
            {warns > 0 && <span class="chip warn">{warns} warning{warns > 1 ? 's' : ''}</span>}
            {diags.length - errors - warns > 0 && <span class="chip">{diags.length - errors - warns} info</span>}
          </>
        )}
      </div>
      {open && diags.length > 0 && (
        <div class="scroll" style={{ minHeight: 0 }}>
          {diags.map((d, i) => {
            const fix = suggestion(d);
            return (
              <div key={i} class="diag" onClick={() => onJump(d)} data-testid="diag">
                <span class={'sev ' + d.severity} />
                <div>
                  <div class="msg">{md(d.message)}</div>
                  {d.hint && <div class="hint">{md(d.hint)}</div>}
                  {fix && (
                    <button
                      class="btn sm"
                      style={{ marginTop: 6, height: 24 }}
                      onClick={(e) => {
                        e.stopPropagation();
                        onFix(d);
                      }}
                      data-testid="quick-fix"
                    >
                      <Wand size={13} /> Use {fix}
                    </button>
                  )}
                </div>
                <span class="loc">
                  {d.code}
                  {d.line > 0 ? ` · ${d.line}:${d.col}` : ''}
                  {d.format ? ` · ${d.format}` : ''}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
