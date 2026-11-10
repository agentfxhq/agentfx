// CodeMirror 6 editor for Reel Script: highlighting, lint squiggles with quick fixes, and one undo
// history for typing and timeline edits (timeline edits arrive as transactions too).
import { defaultKeymap, history, historyKeymap, indentWithTab, redo, undo, redoDepth, undoDepth } from '@codemirror/commands';
import { bracketMatching, indentOnInput } from '@codemirror/language';
import { lintGutter, setDiagnostics, type Diagnostic as CmDiagnostic } from '@codemirror/lint';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { EditorState, Transaction } from '@codemirror/state';
import { drawSelection, EditorView, highlightActiveLine, highlightActiveLineGutter, keymap, lineNumbers } from '@codemirror/view';
import { useEffect, useImperativeHandle, useRef } from 'preact/hooks';
import { forwardRef } from 'preact/compat';
import type { Diagnostic } from '../api';
import { reelLanguage, type Vocab } from './reelLang';

export interface EditorApi {
  setText: (text: string, userEvent?: string) => void;
  undo: () => void;
  redo: () => void;
  jumpTo: (line: number, col: number) => void;
  focus: () => void;
  depth: () => { undo: number; redo: number };
  applyFix: (d: Diagnostic) => boolean;
}

interface Props {
  initial: string;
  diagnostics: Diagnostic[];
  vocab: () => Vocab;
  onChange: (text: string, fromTimeline: boolean) => void;
  onHistory?: (d: { undo: number; redo: number }) => void;
}

/** The text range a diagnostic points at: the token at line/col, or `#id` for line 0. */
export function diagRange(doc: EditorState['doc'], d: Diagnostic): { from: number; to: number } {
  if (d.line > 0 && d.line <= doc.lines) {
    const line = doc.line(d.line);
    if (d.id) {
      const m = new RegExp('#' + escapeRe(d.id.split('.')[0]) + '(?![\\w-])').exec(line.text);
      if (m) return { from: line.from + m.index, to: line.from + m.index + m[0].length };
    }
    const from = Math.min(line.to, line.from + Math.max(0, d.col - 1));
    const rest = line.text.slice(from - line.from);
    const m = /^("([^"\\]|\\.)*"?|[^\s;]+)/.exec(rest);
    return { from, to: Math.max(from + 1, Math.min(line.to, from + (m ? m[0].length : 1))) };
  }
  if (d.id) {
    const id = '#' + d.id.split('.')[0];
    const text = doc.toString();
    const re = new RegExp(escapeRe(id) + '(?![\\w-])');
    const m = re.exec(text);
    if (m) return { from: m.index, to: m.index + id.length };
  }
  return { from: 0, to: Math.min(doc.length, doc.line(1).to) };
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

export function suggestion(d: Diagnostic): string | null {
  const m = /did you mean `([^`]+)`/.exec(d.hint ?? '');
  return m ? m[1] : null;
}

export const CodeEditor = forwardRef<EditorApi, Props>(function CodeEditor({ initial, diagnostics, vocab, onChange, onHistory }, ref) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView>();
  const cb = useRef({ onChange, onHistory });
  cb.current = { onChange, onHistory };

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: initial,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          drawSelection(),
          history(),
          indentOnInput(),
          bracketMatching(),
          highlightSelectionMatches(),
          lintGutter(),
          EditorState.tabSize.of(2),
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
          reelLanguage(vocab),
          EditorView.contentAttributes.of({ 'aria-label': 'Reel Script source', spellcheck: 'false' }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) {
              const fromTl = u.transactions.some((tr) => tr.isUserEvent('input.timeline'));
              cb.current.onChange(u.state.doc.toString(), fromTl);
            }
            if (u.docChanged || u.transactions.length) cb.current.onHistory?.({ undo: undoDepth(u.state), redo: redoDepth(u.state) });
          }),
        ],
      }),
    });
    view.current = v;
    return () => v.destroy();
  }, []);

  // diagnostics -> squiggles (+ quick fix from "did you mean" hints)
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const doc = v.state.doc;
    const cm: CmDiagnostic[] = diagnostics.map((d) => {
      const { from, to } = diagRange(doc, d);
      const fix = suggestion(d);
      return {
        from,
        to,
        severity: d.severity === 'error' ? 'error' : d.severity === 'warning' ? 'warning' : 'info',
        source: d.code,
        message: d.message + (d.hint ? `\n${d.hint}` : ''),
        actions: fix
          ? [{ name: `Use ${fix}`, apply: (view: EditorView, f: number, t: number) => view.dispatch({ changes: { from: f, to: t, insert: fixFor(view.state.sliceDoc(f, t), fix) }, userEvent: 'input.fix' }) }]
          : undefined,
      };
    });
    v.dispatch(setDiagnostics(v.state, cm));
  }, [diagnostics]);

  useImperativeHandle(ref, () => ({
    setText(text, userEvent = 'input.timeline') {
      const v = view.current!;
      const cur = v.state.doc.toString();
      if (cur === text) return;
      // minimal change: common prefix/suffix, so the cursor and history stay small
      let a = 0;
      while (a < cur.length && a < text.length && cur[a] === text[a]) a++;
      let b = 0;
      while (b < cur.length - a && b < text.length - a && cur[cur.length - 1 - b] === text[text.length - 1 - b]) b++;
      v.dispatch({
        changes: { from: a, to: cur.length - b, insert: text.slice(a, text.length - b) },
        userEvent,
        annotations: Transaction.addToHistory.of(true),
      });
    },
    undo: () => void undo(view.current!),
    redo: () => void redo(view.current!),
    jumpTo(line, col) {
      const v = view.current!;
      const l = v.state.doc.line(Math.max(1, Math.min(line, v.state.doc.lines)));
      const pos = Math.min(l.to, l.from + Math.max(0, col - 1));
      v.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
      v.focus();
    },
    focus: () => view.current?.focus(),
    depth: () => ({ undo: undoDepth(view.current!.state), redo: redoDepth(view.current!.state) }),
    applyFix(d) {
      const v = view.current!;
      const fix = suggestion(d);
      if (!fix) return false;
      const { from, to } = diagRange(v.state.doc, d);
      v.dispatch({ changes: { from, to, insert: fixFor(v.state.sliceDoc(from, to), fix) }, userEvent: 'input.fix' });
      return true;
    },
  }));

  return <div ref={host} class="editor-host" data-testid="editor" />;
});

/** Replace the misspelled part of a token with the suggestion, keeping a `#` or `~` prefix etc. */
function fixFor(token: string, fix: string): string {
  if (fix.startsWith('#') || fix.startsWith('@')) return token.startsWith(fix[0]) ? fix : fix.slice(1);
  if (token.startsWith('#') || token.startsWith('@')) return token[0] + fix;
  const eq = token.indexOf('=');
  if (eq > 0 && !fix.includes('=')) return fix + token.slice(eq);
  return fix;
}
