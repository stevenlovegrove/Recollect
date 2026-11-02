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
