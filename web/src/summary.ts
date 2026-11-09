import type { GraphNode, OptionView } from "./api";
import type { Workspace } from "./workspace";
import { BELIEF_MEANS, PRIOR_MEANS, beliefText, casesOf, cellText, reading, truthWords } from "./bits";

/**
 * What a node says beyond its name, a line each: what a map card shows, cut short to fit. `about` says what
 * the line means, where its name alone doesn't, and `component` is the component it's about, if one is.
 */
export interface SummaryLine {
  name: string;
  text: string;
  about?: string;
  component?: string;
  trivial: boolean;
}

/**
 * How strongly each answer to something uncertain is believed, and how strongly it was before the evidence
 * where that's moved it, for an uncertain decision or one of its options. A belief depends on what's
 * pinned, so it's the view's to work out, not a cell's.
 */
export function beliefLines(ws: Workspace, n: GraphNode): SummaryLine[] {
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;
  const vars = ws.view!.worlds.vars;
  const out: SummaryLine[] = [];
  const moved = (o: { chance: number; weight: number }) => beliefText(o.chance) !== beliefText(o.weight);
  // The percentage comes first, so a long answer cut short still reads as one.
  const asked = vars.find((v) => v.id === n.id && v.uncertain);
  if (asked) {
    const list = (p: (o: OptionView) => number) =>
      [...asked.options]
        .filter((o) => p(o) > 0)
        .sort((a, b) => p(b) - p(a))
        .map((o) => `${beliefText(p(o))} ${nameOf(o.id)}`)
        .join(" · ");
    out.push({ name: "belief", text: list((o) => o.chance), about: BELIEF_MEANS, component: "uncertain", trivial: false });
    if (asked.options.some(moved)) out.push({ name: "prior_belief", text: list((o) => o.weight), about: PRIOR_MEANS, component: "uncertain", trivial: false });
  }
  const answer = vars.find((v) => v.uncertain && v.options.some((o) => o.id === n.id));
  if (answer) {
    const o = answer.options.find((x) => x.id === n.id)!;
    out.push({ name: "belief", text: beliefText(o.chance), about: BELIEF_MEANS, component: "option", trivial: o.chance === 0 });
    if (moved(o)) out.push({ name: "prior_belief", text: beliefText(o.weight), about: PRIOR_MEANS, component: "option", trivial: false });
  }
  return out;
}

/** A cell in a line, the way a card says it. */
export function cellLine(ws: Workspace, n: GraphNode, c: string): SummaryLine | null {
  const cell = n.cells[c];
  if (!cell) return null;
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;
  const r = reading(cell);
  const text = cellText(cell, nameOf);
  const words = r && truthWords(c, r);
  const reads = !words ? text : "fixed" in words ? words.fixed : `${words.lead} ${text}`;
  const info = ws.cellInfo(c);
  return { name: c, text: reads, about: info?.about, component: info?.component, trivial: !!r?.never };
}

function summaryLines(ws: Workspace, n: GraphNode): SummaryLine[] {
  const beliefs = beliefLines(ws, n);
  const out = [...beliefs];
  for (const c of Object.keys(n.cells)) {
    const info = ws.cellInfo(c);
    const r = reading(n.cells[c]);
    if (info) {
      if (!info.summary) continue;
      // How strongly it's believed says more than when it's in play.
      if (c === "need" && beliefs.some((b) => b.component === "option")) continue;
      if (r && (r.always || (c === "actionable" && r.never))) continue;
    } else {
      // The workspace's own cells that are worked out are what it's for, so they say something too. A kind's
      // own, with nothing of its own to work them out from, don't.
      const cell = n.cells[c];
      if (cell.formula === undefined || cell.error || casesOf(cell).every((v) => v === null)) continue;
    }
    const line = cellLine(ws, n, c);
    if (line) out.push(line);
  }
  return out;
}

/** A node's lines with those of the rows that are the same thing, each name once. */
export function summaryOf(ws: Workspace, node: GraphNode): SummaryLine[] {
  return [node, ...(node.parts ?? []).flatMap((p) => ws.byId.get(p) ?? [])].flatMap((n) => summaryLines(ws, n)).filter((l, i, all) => all.findIndex((x) => x.name === l.name) === i);
}
