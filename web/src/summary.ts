import type { GraphNode } from "./api";
import type { Workspace } from "./workspace";
import { LIKELY_MEANS, casesOf, cellText, likelyText, reading, truthWords } from "./bits";

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
  // Something uncertain is answered by how likely each way is, and each way by how likely it is: that
  // says more than when it's in play. The chance comes first, so a long answer cut short still reads as one.
  const vars = g.worlds.vars;
  const asked = vars.find((v) => v.id === n.id && v.uncertain);
  if (asked) {
    const text = [...asked.options]
      .filter((o) => o.chance > 0)
      .sort((a, b) => b.chance - a.chance)
      .map((o) => `${likelyText(o.chance)} ${nameOf(o.id)}`)
      .join(" · ");
    out.push({ name: "likely", text, about: LIKELY_MEANS, trivial: false });
  }
  const answer = vars.find((v) => v.uncertain && v.options.some((o) => o.id === n.id));
  if (answer) {
    const o = answer.options.find((x) => x.id === n.id)!;
    out.push({ name: "likely", text: likelyText(o.chance), about: LIKELY_MEANS, trivial: o.chance === 0 });
  }
  for (const c of g.components) {
    if (!c.summary) continue;
    // How likely it is says more than when it's in play.
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
