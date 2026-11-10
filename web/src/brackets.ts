const CLOSING: Record<string, string> = { "(": ")", "{": "}", "[": "]" };
const OPENING: Record<string, string> = { ")": "(", "}": "{", "]": "[" };

/** A stretch of text, and the bracket pairs around it from the outermost in. A pair is known by where it opens. */
export type BracketRun = { text: string; bracket: boolean; held: number[] };

/** The text of a pair of brackets, including them, by where it opens. */
export function pairText(text: string, runs: BracketRun[], open: number): string {
  let at = 0;
  for (const r of runs) {
    if (r.bracket && at > open && r.held[r.held.length - 1] === open) return text.slice(open, at + 1);
    at += r.text.length;
  }
  return text.slice(open);
}

/**
 * What a pair of brackets holds as a part that means something on its own. Round brackets do already. A
 * row's address, `Pet[Fox]`, means nothing without its table, and what's read through it, `.chosen.value`,
 * is usually what's wanted, so square brackets take in the name before them and the fields after.
 */
export function partText(text: string, runs: BracketRun[], open: number): string {
  const pair = pairText(text, runs, open);
  if (text[open] !== "[") return pair;
  const before = /[\p{L}\p{N}_.]*$/u.exec(text.slice(0, open))?.[0] ?? "";
  if (!before) return pair;
  const after = /^(?:\.[\p{L}_][\p{L}\p{N}_]*)*/u.exec(text.slice(open + pair.length))?.[0] ?? "";
  return before.replace(/^\./, "") + pair + after;
}

/** Text cut up by its brackets. One with no partner, say in somebody's name, is only text. */
export function bracketRuns(text: string): BracketRun[] {
  const open: number[] = [];
  const opens = new Set<number>();
  const closes = new Map<number, number>();
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (CLOSING[c]) open.push(i);
    else if (OPENING[c]) {
      const at = open.findLastIndex((o) => text[o] === OPENING[c]);
      if (at < 0) continue;
      opens.add(open[at]);
      closes.set(i, open[at]);
      open.length = at;
    }
  }

  const runs: BracketRun[] = [];
  const held: number[] = [];
  let from = 0;
  const flush = (to: number) => {
    if (to > from) runs.push({ text: text.slice(from, to), bracket: false, held: [...held] });
  };
  for (let i = 0; i < text.length; i++) {
    const isOpen = opens.has(i);
    if (!isOpen && !closes.has(i)) continue;
    flush(i);
    if (isOpen) held.push(i);
    runs.push({ text: text[i], bracket: true, held: [...held] });
    if (!isOpen) held.pop();
    from = i + 1;
  }
  flush(text.length);
  return runs;
}
