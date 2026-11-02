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
  completion: "Completion",
  blocked: "Waiting",
  blockers: "Waits for",
  actionable: "Can start",
  scope: "Inside",
};

export const label = (c: string) => LABEL[c] ?? (c[0]?.toUpperCase() ?? "") + c.slice(1).replace(/_/g, " ");

/** The stored facts that say what a node is, which is what its badges show. */
const IGNORED = ["name", "body", "sources", "estimate"];

export function factsOf(n: GraphNode): string[] {
  return Object.entries(n.cells)
    .filter(([c, cell]) => !IGNORED.includes(c) && (cell.own || cell.source === "stored"))
    .map(([c]) => c);
}

const ORDER = ["goal", "decision", "risk", "question", "unknown", "requires", "domain", "task", "macro", "value"];

/** The component that best characterises a node, for colouring. */
export function dominant(n: GraphNode): string {
  const facts = factsOf(n);
  return ORDER.find((c) => facts.includes(c)) ?? "none";
}

export const nodeColor = (n: GraphNode) => compColor(dominant(n));

export const isDropped = (n: GraphNode) => "dropped" in n.cells;

export const pct = (x: number) => `${Math.round(x * 100)}%`;
