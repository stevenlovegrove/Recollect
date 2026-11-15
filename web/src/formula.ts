import type { GraphNode } from "./api";

/** Words that mean something in a formula already, so they're never a name to label. */
export const NOT_NAMES = new Set(["true", "false", "undefined", "self", "from", "to", "via", "in", "out", "with", "if", "defined", "chose", "round", "floor", "ceil", "sum", "product", "all", "any", "count", "pick", "distinct", "row", "rows", "prev", "next", "abs", "min", "max", "or", "has", "whole", "is_leaf", "done_beneath", "days", "exp", "ln", "sqrt", "normal", "lognormal", "uniform", "triangular", "exponential", "between", "integers", "bernoulli"]);

/** The names a formula uses that have to come from somewhere: its labels. */
export function namesIn(formula: string): string[] {
  const text = formula.replace(/"(\\.|[^"\\])*"/g, '""');
  const bound = new Set([...text.matchAll(/([\p{L}_][\p{L}\p{N}_]*)\s+in\b/gu)].map((m) => m[1]));
  const names = [...text.matchAll(/(?<![\p{L}\p{N}_.])([\p{L}_][\p{L}\p{N}_]*)/gu)].map((m) => m[1]);
  return [...new Set(names)].filter((n) => !NOT_NAMES.has(n) && !bound.has(n));
}

/**
 * The node a name in a formula most likely means: the one whose name it is, ignoring case and reading _ as
 * a space, or failing that the one whose name starts with it, if only one does. Otherwise it's asked.
 */
export function guessNode(nodes: GraphNode[], name: string): GraphNode | null {
  const want = name.replace(/_/g, " ").toLowerCase();
  const exact = nodes.filter((n) => n.name.toLowerCase() === want);
  if (exact.length === 1) return exact[0];
  if (exact.length > 1) return null;
  const starts = nodes.filter((n) => n.name.toLowerCase().startsWith(want));
  return starts.length === 1 ? starts[0] : null;
}
