// Apply a JSON-form edit to the author's text without reformatting the whole file.
//
// The timeline edits the JSON form; the CLI turns JSON into canonical text. Canonical text drops
// comments and normalises quoting, so instead of replacing the author's file we diff canonical(old)
// against canonical(new) and carry the changed lines over to the matching lines of the original,
// keeping their indentation and trailing comments. When the lines cannot be matched one to one
// (for example `;`-joined statements written on several lines), we fall back to the canonical text.

export interface PatchResult {
  text: string;
  normalized: boolean; // true: fell back to the canonical text (comments and formatting lost)
}

function splitComment(line: string): [string, string] {
  let q = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === '\\' && q) {
      i++;
      continue;
    }
    if (c === '"') q = !q;
    else if (!q && c === '/' && line[i + 1] === '/' && (i === 0 || /\s/.test(line[i - 1]))) {
      let j = i;
      while (j > 0 && /\s/.test(line[j - 1])) j--;
      return [line.slice(0, j), line.slice(j)];
    }
  }
  return [line, ''];
}

/** Tokens of a statement line, quotes stripped and numbers normalised, for matching. */
export function lineTokens(line: string): string {
  const [code] = splitComment(line);
  const toks: string[] = [];
  let cur = '';
  let q = false;
  let depth = 0;
  for (let i = 0; i < code.length; i++) {
    const c = code[i];
    if (q) {
      if (c === '\\') {
        cur += code[++i] ?? '';
        continue;
      }
      if (c === '"') {
        q = false;
        continue;
      }
      cur += c;
      continue;
    }
    if (c === '"') {
      q = true;
      continue;
    }
    if (c === '(' || c === '[') depth++;
    if (c === ')' || c === ']') depth--;
    if (/\s/.test(c) && depth <= 0) {
      if (cur) toks.push(cur);
      cur = '';
      continue;
    }
    cur += c;
  }
  if (cur) toks.push(cur);
  const norm = toks.map((t) => {
    const m = /^([\w-]+=)?(-?\d+(?:\.\d+)?)$/.exec(t);
    return m ? (m[1] ?? '') + String(Number(m[2])) : t.replace(/\s+/g, ' ');
  });
  // the canonical text writes bare args (`nowrap`) before key=value props: compare props as a set
  const isProp = (t: string) => /^[\w.-]+=/.test(t);
  return [...norm.filter((t) => !isProp(t)), ...norm.filter(isProp).sort()].join(' ');
}

const indentOf = (l: string) => /^\s*/.exec(l)![0];

export function patchSource(original: string, canonOld: string, canonNew: string): PatchResult {
  if (canonOld === canonNew) return { text: original, normalized: false };
  const trailingNl = original.endsWith('\n');
  const O = original.replace(/\n$/, '').split('\n');
  const A = canonOld.replace(/\n$/, '').split('\n');
  const B = canonNew.replace(/\n$/, '').split('\n');
  let pre = 0;
  while (pre < A.length && pre < B.length && A[pre] === B[pre]) pre++;
  let suf = 0;
  while (suf < A.length - pre && suf < B.length - pre && A[A.length - 1 - suf] === B[B.length - 1 - suf]) suf++;
  const aChanged = A.slice(pre, A.length - suf);
  const bChanged = B.slice(pre, B.length - suf);

  const fallback = { text: canonNew.endsWith('\n') || !trailingNl ? canonNew : canonNew + '\n', normalized: true };

  // statement lines of the original (skip blanks and comment-only lines), with their token keys
  const oIdx: number[] = [];
  const oKeys: string[] = [];
  O.forEach((l, i) => {
    const k = lineTokens(l);
    if (k) {
      oIdx.push(i);
      oKeys.push(k);
    }
  });
  const aKeys = A.map(lineTokens);
  // the original must have the same statements as canonical(old), line for line
  if (oKeys.length !== aKeys.length || oKeys.some((k, i) => k !== aKeys[i])) return fallback;

  const out = O.slice();
  if (aChanged.length === bChanged.length) {
    aChanged.forEach((_, k) => {
      const oi = oIdx[pre + k];
      const [, comment] = splitComment(O[oi]);
      const ind = indentOf(O[oi]);
      out[oi] = ind + bChanged[k].trimStart() + comment;
    });
  } else {
    // lines inserted or removed: splice them in, indenting like the canonical text relative to its neighbour
    const anchorO = pre < oIdx.length ? oIdx[pre] : O.length;
    const startO = aChanged.length ? oIdx[pre] : anchorO;
    const endO = aChanged.length ? oIdx[pre + aChanged.length - 1] + 1 : startO;
    const unit = /^\t/.test(O.find((l) => /^\s+\S/.test(l)) ?? '') ? '\t' : '  ';
    const reindent = (l: string) => {
      const lvl = indentOf(l).length / 2;
      return unit.repeat(lvl) + l.trimStart();
    };
    out.splice(startO, endO - startO, ...bChanged.map(reindent));
  }
  return { text: out.join('\n') + (trailingNl ? '\n' : ''), normalized: false };
}
