import type { GraphNode } from "./api";

// Colours are CSS variables (see styles.css) so they follow light/dark mode. A component a system
// doesn't colour here still works: it just gets the neutral colour. Its icon comes from the system.
const COLOURED = ["goal", "decision", "task", "question", "risk", "unknown", "value", "options", "requires", "macro"];

export const compColor = (c: string) => (COLOURED.includes(c) ? `var(--c-${c})` : "var(--c-none)");

/** Plain words for the components and cells people deal with. Anything else reads by its name. */
const LABEL: Record<string, string> = {
  part_of: "part of",
  depends_on: "depends on",
  is_a: "is a",
  need: "Needed",
  blocked: "Waiting",
  actionable: "Can start",
  wait_for: "Waits until",
  ruled_out: "Ruled out",
  called: "Called",
  body: "Notes",
};

export const label = (c: string) => LABEL[c] ?? (c[0]?.toUpperCase() ?? "") + c.slice(1).replace(/_/g, " ");

/** What a node can have that says nothing about what it is: words about it, and where it came from. */
const ASIDES = ["notes", "sources"];

/** The components that say what a node is, which is what its badges show: what it was given, or takes from its kind. */
export function badgesOf(n: GraphNode): string[] {
  return n.components.filter((c) => (c.why === "given" || c.why === "kind") && !ASIDES.includes(c.name)).map((c) => c.name);
}

/** Says nothing but words: no components of its own, and no cells of the workspace's own. */
export const isNote = (n: GraphNode) => !badgesOf(n).length && !n.components.some((c) => c.why === "edge");

const ORDER = ["goal", "decision", "risk", "question", "unknown", "requires", "options", "task", "macro", "value"];

/** The component that best characterises a node, for colouring. */
export function dominant(n: GraphNode): string {
  const has = n.components.map((c) => c.name);
  return ORDER.find((c) => has.includes(c)) ?? "none";
}

export const nodeColor = (n: GraphNode) => compColor(dominant(n));

export const isDropped = (n: GraphNode) => n.cells.dropped?.value === true;

export const pct = (x: number) => `${Math.round(x * 100)}%`;
