import type { GraphNode } from "./api";

// Colours are CSS variables (see styles.css) so they follow light/dark mode. A component a system
// doesn't describe here still works: it just gets the neutral colour and no glyph.
const COLOURED = ["goal", "decision", "task", "question", "risk", "unknown", "value", "domain", "requires", "macro"];

export const compColor = (c: string) => (COLOURED.includes(c) ? `var(--c-${c})` : "var(--c-none)");

export const GLYPH: Record<string, string> = {
  goal: "◎",
  decision: "⑂",
  task: "▸",
  question: "?",
  risk: "!",
  unknown: "…",
  done: "✓",
  dropped: "✕",
  value: "=",
  domain: "◇",
  requires: "✦",
  macro: "ƒ",
  estimate: "⏱",
  drawing: "✎",
};

/** Plain words for the components people deal with. Anything else reads by its name. */
const LABEL: Record<string, string> = {
  part_of: "part of",
  depends_on: "depends on",
  need: "Needed",
  blocked: "Waiting",
  blockers: "Waits for",
  actionable: "Can start",
  scope: "Inside",
  domain: "Options",
};

export const label = (c: string) => LABEL[c] ?? (c[0]?.toUpperCase() ?? "") + c.slice(1).replace(/_/g, " ");

/** The stored facts that say what a node is, which is what its badges show. A price says nothing about that. */
const IGNORED = ["name", "body", "sources", "estimate"];

export function factsOf(n: GraphNode): string[] {
  return Object.entries(n.cells)
    .filter(([c, cell]) => !IGNORED.includes(c) && !cell.workspace && (cell.own || cell.source === "stored"))
    .map(([c]) => c);
}

/** Says nothing but words: no facts, and no cells of the workspace's own, like a car's price or a kind's formula. */
export const isNote = (n: GraphNode) => !factsOf(n).length && !Object.values(n.cells).some((c) => c.workspace && c.own);

const ORDER = ["goal", "decision", "risk", "question", "unknown", "requires", "domain", "task", "macro", "value"];

/** The component that best characterises a node, for colouring. */
export function dominant(n: GraphNode): string {
  const facts = factsOf(n);
  return ORDER.find((c) => facts.includes(c)) ?? "none";
}

export const nodeColor = (n: GraphNode) => compColor(dominant(n));

export const isDropped = (n: GraphNode) => n.cells.dropped?.value === true;

export const pct = (x: number) => `${Math.round(x * 100)}%`;
