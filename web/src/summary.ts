import type { GraphNode, OptionView } from "./api";
import type { Workspace } from "./workspace";
import { BELIEF_MEANS, PRIOR_MEANS, beliefText, casesOf, cellText, reading, truthWords } from "./bits";

/**
 * What a node says beyond its name, a line each: what a map card shows, cut short to fit, and what the
 * inspector shows in full. `about` says what the line means, where its name alone doesn't.
 */
export interface SummaryLine {
  name: string;
  text: string;
  about?: string;
  trivial: boolean;
}

export function summaryLines(ws: Workspace, n: GraphNode): SummaryLine[] {
  const g = ws.view!;
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;
  const out: SummaryLine[] = [];
  // Something uncertain is answered by how strongly each way is believed, and each way by how strongly it
  // is: that says more than when it's in play. The percentage comes first, so a long answer cut short still
  // reads as one. What was believed before the evidence is said too, where the evidence has moved it, so
  // it's plain how much of a belief is the evidence and how much was there to begin with.
  const vars = g.worlds.vars;
  const moved = (o: { chance: number; weight: number }) => beliefText(o.chance) !== beliefText(o.weight);
  const asked = vars.find((v) => v.id === n.id && v.uncertain);
  if (asked) {
    const list = (p: (o: OptionView) => number) =>
      [...asked.options]
        .filter((o) => p(o) > 0)
        .sort((a, b) => p(b) - p(a))
        .map((o) => `${beliefText(p(o))} ${nameOf(o.id)}`)
        .join(" · ");
    out.push({ name: "belief", text: list((o) => o.chance), about: BELIEF_MEANS, trivial: false });
    if (asked.options.some(moved)) out.push({ name: "prior_belief", text: list((o) => o.weight), about: PRIOR_MEANS, trivial: false });
  }
  const answer = vars.find((v) => v.uncertain && v.options.some((o) => o.id === n.id));
  if (answer) {
    const o = answer.options.find((x) => x.id === n.id)!;
    out.push({ name: "belief", text: beliefText(o.chance), about: BELIEF_MEANS, trivial: o.chance === 0 });
    if (moved(o)) out.push({ name: "prior_belief", text: beliefText(o.weight), about: PRIOR_MEANS, trivial: false });
  }
  for (const c of g.components) {
    if (!c.summary) continue;
    // How strongly it's believed says more than when it's in play.
    if (answer && c.name === "need") continue;
    const cell = n.cells[c.name];
    if (!cell) continue;
    const r = reading(cell);
    if (r && (r.always || (c.name === "actionable" && r.never))) continue;
    const text = cellText(cell, nameOf);
    const words = r && truthWords(c.name, r);
    const reads = !words ? text : "fixed" in words ? words.fixed : `${words.lead} ${text}`;
    out.push({ name: c.name, text: reads, about: c.about, trivial: !!r?.never });
  }
  // The workspace's own cells that are worked out are what it's for, so they say something too. A kind's
  // own, with nothing of its own to work them out from, don't.
  for (const [c, cell] of Object.entries(n.cells)) {
    if (!cell.workspace || cell.formula === undefined || cell.error || casesOf(cell).every((v) => v === null)) continue;
    out.push({ name: c, text: cellText(cell, nameOf), trivial: false });
  }
  return out;
}

/** A node's lines with those of the rows that are the same thing, each name once. */
export function summaryOf(ws: Workspace, node: GraphNode): SummaryLine[] {
  return [node, ...(node.parts ?? []).flatMap((p) => ws.byId.get(p) ?? [])].flatMap((n) => summaryLines(ws, n)).filter((l, i, all) => all.findIndex((x) => x.name === l.name) === i);
}
