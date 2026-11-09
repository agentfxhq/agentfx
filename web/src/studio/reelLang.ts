// A small Reel Script highlighter for CodeMirror 6 (stream parser) + completions.
import { autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { HighlightStyle, StreamLanguage, syntaxHighlighting, type StringStream } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import type { Extension } from '@codemirror/state';

interface State {
  stmt: boolean; // at the start of a statement (kind expected)
  afterEq: boolean;
  rawIndent: number | null; // inside a `|` raw body deeper than this indent
  lineIndent: number;
}

const reel = StreamLanguage.define<State>({
  name: 'reel',
  startState: () => ({ stmt: true, afterEq: false, rawIndent: null, lineIndent: 0 }),
  token(stream: StringStream, st: State) {
    if (stream.sol()) {
      st.stmt = true;
      st.afterEq = false;
      st.lineIndent = stream.indentation();
      if (st.rawIndent !== null) {
        if (st.lineIndent > st.rawIndent) {
          stream.skipToEnd();
          return 'meta';
        }
        st.rawIndent = null;
      }
    }
    if (stream.eatSpace()) {
      st.afterEq = false;
      return null;
    }
    const ch = stream.peek()!;
    if (ch === '/' && stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }
    if (ch === '"') {
      stream.next();
      let esc = false;
      let c: string | void;
      while ((c = stream.next()) !== undefined) {
        if (c === '"' && !esc) break;
        esc = !esc && c === '\\';
      }
      st.stmt = false;
      st.afterEq = false;
      return 'string';
    }
    if (ch === ';') {
      stream.next();
      st.stmt = true;
      return 'operator';
    }
    if (ch === '|' && /^\|\s*(\/\/.*)?$/.test(stream.string.slice(stream.pos))) {
      stream.next();
      st.rawIndent = st.lineIndent;
      return 'operator';
    }
    if (st.stmt && ch === '@') {
      stream.match(/^@[^\s;]+/);
      return 'labelName';
    }
    if (st.stmt && stream.match(/^[A-Za-z][\w-]*/)) {
      st.stmt = false;
      return 'keyword';
    }
    st.stmt = false;
    if (stream.match(/^[A-Za-z_][\w.-]*(?==)/)) return 'propertyName';
    if (ch === '=') {
      stream.next();
      st.afterEq = true;
      return 'operator';
    }
    if (ch === '#') {
      stream.match(/^#[\w.-]+/);
      return /^#[0-9A-Fa-f]{3,8}$/.test(stream.current()) && st.afterEq ? 'atom' : 'variableName';
    }
    if (ch === '$') {
      stream.match(/^\$[\w-]+/);
      return 'variableName.special';
    }
    if (ch === '~') {
      stream.next();
      stream.match(/^[\w.-]+(\([^)]*\))?/);
      return 'ease';
    }
    if (stream.match(/^-?\d+(\.\d+)?(ms|s|f|%|px|deg|x|vw|vh|em)?/) || stream.match(/^-?\.\d+(ms|s|f|%)?/)) return 'number';
    if (ch === ':' || ch === ',' || ch === '[' || ch === ']' || ch === '(' || ch === ')') {
      stream.next();
      return 'punctuation';
    }
    stream.match(/^[^\s;=:~"]+/) || stream.next();
    return null;
  },
  languageData: { commentTokens: { line: '//' } },
  tokenTable: { ease: t.function(t.variableName), punctuation: t.punctuation },
});

const style = HighlightStyle.define([
  { tag: t.keyword, class: 'tok-kind' },
  { tag: t.labelName, class: 'tok-anchor' },
  { tag: t.variableName, class: 'tok-id' },
  { tag: t.special(t.variableName), class: 'tok-token' },
  { tag: t.propertyName, class: 'tok-key' },
  { tag: t.string, class: 'tok-str' },
  { tag: t.number, class: 'tok-num' },
  { tag: t.atom, class: 'tok-num' },
  { tag: t.comment, class: 'tok-comment' },
  { tag: t.function(t.variableName), class: 'tok-ease' },
  { tag: t.meta, class: 'tok-raw' },
]);

export interface Vocab {
  kinds: string[];
  channels: string[];
  eases: string[];
}

export function reelLanguage(vocab: () => Vocab): Extension[] {
  const complete = (ctx: CompletionContext): CompletionResult | null => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = line.text.slice(0, ctx.pos - line.from);
    const v = vocab();
    let m = /~([\w.-]*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: v.eases.map((e) => ({ label: e, type: 'function' })), validFor: /^[\w.()-]*$/ };
    m = /(?:^\s*|;\s*|^\s*@\S+\s+)([a-z][\w-]*)$/.exec(before);
    if (m && (m[1].length > 0 || ctx.explicit)) return { from: ctx.pos - m[1].length, options: v.kinds.map((k) => ({ label: k, type: 'keyword' })), validFor: /^[\w-]*$/ };
    m = /\banim\s+(?:#[\w-]+\s+)?(?:\S+\s+)*([a-z][\w-]*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: v.channels.map((c) => ({ label: c, type: 'property' })), validFor: /^[\w-]*$/ };
    return null;
  };
  return [reel, syntaxHighlighting(style), autocompletion({ override: [complete], icons: false })];
}
