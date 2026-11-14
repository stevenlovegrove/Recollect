// Types mirror what recollect-core sends, whether from the server or from the engine running here.

import { ApiError, backend } from "./backend";

export type World = Record<string, string>;

export interface Reading {
  when: string;
  prob: number;
  always: boolean;
  never: boolean;
}

export type Shown =
  | ({ kind: "bool" } & Reading)
  | { kind: "cond"; cases: { when: Reading; value: unknown }[] }
  | { kind: "named"; items: { node: string; when: Reading }[] };

/**
 * What an uncertain number comes to, sent as `{"dist": …}` wherever the number would be. See
 * docs/distributions.md.
 */
export interface Dist {
  mean: number;
  /** Its spread: the standard deviation. */
  sd: number;
  /** It's below this one time in twenty. */
  low: number;
  /** It's below this half the time. */
  mid: number;
  /** It's above this one time in twenty. */
  high: number;
  /** It's never below this, or above that, where that's known. */
  least?: number;
  most?: number;
  /** How much of the time it's undefined. The rest is about when it isn't. */
  undefined: number;
  hist: { from: number; to: number; bars: number[] };
  /** How much of the time it's below 0, where it can be. */
  below0?: number;
  /** Where it's only ever one of a few numbers: each, and how likely it is, in order. */
  points?: [number, number][];
  /** The distribution it is, if it has a name, as it would be written. */
  law?: string;
  /** These are exactly right. Otherwise they were read from `draws` draws, and `error` is how far out the mean could be. */
  exact: boolean;
  draws: number;
  error: number;
}

/** A number read from draws, such as a mean with no law, sent as `{"estimate": …}` wherever the number would be. */
export interface Estimate {
  value: number;
  /** How far out it could be: its standard error. Missing where it's come through something that doesn't say. */
  error?: number | null;
  draws: number;
}

/** How likely an uncertain truth is, sent as `{"chance": …}` wherever the truth would be. */
export interface Chance {
  chance: number;
  undefined: number;
  exact: boolean;
  draws: number;
  /** In how many of the draws it was true. */
  hits: number;
  error: number;
}

export interface Cell {
  /** Where what's in it comes from: written here, taken from a kind, or its component's default. */
  source: "own" | "kind" | "default";
  /** The kind it's taken from, if it is. */
  kind?: string;
  /** The kind that would say what it is, if this didn't. */
  beneath?: string;
  /** The formula, if one backs it. */
  formula?: string;
  value?: unknown;
  shown?: Shown;
  /** The open choices it differs over, by id. Empty if it's the same in every world that's left. */
  vars?: string[];
  /** Whether it differs in any world at all, whatever's pinned or ruled out. */
  varies?: boolean;
  /** How likely it is, at the least and the most, over the ways the open choices that are yours could go. */
  over?: { low: number; high: number };
  /** Uncertain, and differing only over what's a matter of chance: what it comes to with all of that taken together. */
  mix?: unknown;
  error?: string;
}

export interface RowCells {
  key: number;
  id: string;
  /** Someone wrote it. Otherwise it's worked out from the table, and isn't saved. */
  written: boolean;
  cells: Record<string, Cell>;
}

export interface CellRef {
  node: string;
  cell: string;
}

/** Why an entity has a component. */
export interface Has {
  name: string;
  why: "given" | "everywhere" | "kind" | "requires" | "edge";
  /** The kind or edge it comes from, or the component that requires it. */
  from?: string;
}

export interface GraphNode {
  id: string;
  name: string;
  components: Has[];
  cells: Record<string, Cell>;
  /** Which row of which table its name makes it: `Year[3]` is row 3 of Year. */
  row?: { table: string; key: number | string };
  /** The rows of related tables with the same key: with this one, they're one thing. */
  parts?: string[];
  /** The rows its cells read by where they are, as `prev` or `Year[3]` do, rather than through an edge. */
  reads?: string[];
  /** A row nobody wrote, worked out from its table and not saved, here because something written reads it. */
  unwritten?: boolean;
}

export interface Edge {
  id: string;
  from: string;
  to: string;
  components: string[];
  cells: Record<string, Cell>;
}

export interface ComponentInfo {
  name: string;
  system: string;
  place: "node" | "edge" | "whole";
  about: string;
  icon?: string;
  requires: string[];
  /** For an edge: what its source has because of it. */
  source: string[];
  /** For an edge: what its target has because of it. */
  target: string[];
  /** It says something about one entity alone, so it isn't taken from a kind. */
  per_entity: boolean;
  /** Every node has it. */
  everywhere: boolean;
  /** An edge that gives a graph its shape. */
  arranges: boolean;
  /** For an edge that gives a graph its shape: what it points at goes beneath, not above. */
  reversed: boolean;
  /** An edge that says what comes first, which a map can be laid out by. */
  orders: boolean;
  cells: string[];
}

export interface CellInfo {
  name: string;
  component: string;
  about: string;
  internal: boolean;
  /** Worth a line on a card. */
  summary: boolean;
  /** For a view to read, rather than a person. */
  quiet: boolean;
  /** It's either true or false, so it can be read and set as a tick. */
  truth: boolean;
  per_entity: boolean;
  /** What a new one starts as. */
  initial: unknown;
  /** Worked out, unless something says otherwise. */
  computed: boolean;
  /** It can hold a formula. */
  formulas: boolean;
  /** Anything can be written in it. */
  settable: boolean;
}

export interface OptionView {
  id: string;
  name: string;
  weight: number;
  gap: boolean;
  possible: boolean;
  picked: boolean;
  /** How strongly it's believed now, given what's pinned and what rules worlds out. */
  chance: number;
}

/** Another open question, and how sure of this one knowing it would make you, on average. */
export interface ClueView {
  var: string;
  sure: number;
}

export interface VarView {
  id: string;
  name: string;
  options: OptionView[];
  /** Not anyone's to choose, so what's expected averages over it. */
  uncertain: boolean;
  /** For something uncertain that's still open: the chance of its likeliest answer now. */
  sure?: number;
  /** What would help most to find out, best first. */
  clues?: ClueView[];
}

export interface WorldsView {
  picks: World;
  vars: VarView[];
  count: string;
  constrained: boolean;
  solutions: World[];
  /** Quantities pinned to a value or a range, keyed `=node/cell`, and why any couldn't be. */
  quantities?: { key: string; node: string; cell: string; text: string; refused?: string }[];
}

export interface Check {
  severity: "broken" | "notice";
  rule: string;
  on: string[];
  detail: string;
}

export interface View {
  /** `id` is what the app knows it by, and `location` says where it is, for a person to read. */
  workspace: { name: string; about: string | null; id: string; location: string; systems: string[]; sample: string | null };
  /** The newest event since it was opened, which changes with every change. */
  last_seq: number | null;
  /** The event undo would take back. */
  undo: number | null;
  /** The undo redo would take back. */
  redo: number | null;
  nodes: GraphNode[];
  /** Rows nobody wrote that a node reads, so a map can show them. They aren't nodes anywhere else. */
  unwritten: GraphNode[];
  /** Nodes nobody wrote that are only what they say, such as a puzzle choice's options. */
  constants: GraphNode[];
  edges: Edge[];
  components: ComponentInfo[];
  cells: CellInfo[];
  worlds: WorldsView;
  checks: Check[];
  ran: { node: string; cell: string }[];
  proposals: number;
}

export interface Blocker {
  id: string;
  name: string;
  when: string;
}

export type Standing = "now" | "depends" | "waiting" | "finished" | "not_needed";

export interface WorkItem {
  id: string;
  name: string;
  standing: Standing;
  estimate: string | null;
  need: Reading;
  actionable: Reading;
  blockers: Blocker[];
  scope: Blocker | null;
}

export interface MatrixOption {
  id: string;
  name: string;
  weight: number;
  gap: boolean;
  ruled_out: boolean;
  dropped: boolean;
  chosen: boolean;
  /** One the decision lists by name, with no node or link of its own. */
  listed?: boolean;
}

export interface MatrixRow {
  id: string;
  name: string;
  depth: number;
  per_option: Record<string, number>;
  need: Reading;
  finished: boolean;
  is_work: boolean;
  actionable: Reading | null;
  blockers: Blocker[];
}

export interface Matrix {
  id: string;
  name: string;
  chosen: string | null;
  options: MatrixOption[];
  rows: MatrixRow[];
  pressure: { state: "free" | "blocking" | "idle" | "decided"; work: string[] };
}

export interface PlanData {
  work: WorkItem[];
  decisions: Matrix[];
}

export interface OpView {
  verb: "create" | "delete" | "add" | "remove" | "link" | "unlink" | "set" | "clear";
  node: string | null;
  /** For an edge, its target. */
  other: string | null;
  /** The component or cell. */
  what: string | null;
  value: unknown;
  /** What's set is a formula. */
  formula?: boolean;
}

export interface EventView {
  seq: number;
  /** The event this one takes back, if it's an undo or a redo. */
  reverts: number | null;
  at: number;
  author: string;
  why: string;
  ops: OpView[];
}

export interface Proposal {
  id: string;
  title: string;
  rationale: string;
  author: string;
  created: number;
  error: string | null;
  ops: OpView[];
}

export interface FileInfo {
  path: string;
  size: number;
}

export interface FileText extends FileInfo {
  text: string | null;
  truncated: boolean;
}

export interface SampleInfo {
  name: string;
  title: string;
  about: string;
  group: string;
  systems: string[];
}

/** A workspace this browser knows, besides the samples. */
export interface KnownWorkspace {
  id: string;
  name: string;
  kind: "browser" | "folder";
  location: string;
  opened: number;
}

export interface Workspaces {
  current: { name: string; id: string; location: string; sample: string | null; kind?: "browser" | "folder"; readOnly?: boolean } | null;
  samples: SampleInfo[];
  systems: { name: string; about: string }[];
  /** Served by `recollect serve`: where it keeps its own things. */
  home?: string;
  /** Run in the browser: what it's opened before. */
  recent?: KnownWorkspace[];
}

/** What a formula calls which nodes. Several nodes under one name are a group. */
export type Labels = Record<string, string[]>;

/** What a formula would read as, with nothing changed. */
export interface Tried {
  shown: Shown | null;
  expansion: string | null;
  error: string | null;
}

/** A cell as it's given: a value, or a formula with what its names mean. */
export type Written = unknown | { formula: string; labels?: Labels };

/** A change to make. See crates/recollect-core/src/ops.rs for what these can say. */
export type Op =
  | { op: "add_node"; id: string; name?: string; body?: string; components?: string[]; cells?: Record<string, Written>; links?: { to: string; component: string; cells?: Record<string, Written> }[] }
  | { op: "add" | "remove"; id: string; component: string }
  | { op: "set"; id: string; cell: string; value?: unknown; formula?: string; labels?: Labels }
  | { op: "set"; from: string; to: string; cell: string; value?: unknown; formula?: string }
  | { op: "link"; from: string; to: string; component: string; cells?: Record<string, Written> }
  | { op: "unlink"; from: string; to: string; component: string }
  | { op: "delete_node"; id: string };

export const isNothingOpen = (e: unknown) => e instanceof ApiError && e.status === 409;

const call = async <T>(method: string, args: unknown = {}) => (await backend()).call<T>(method, args);

export const api = {
  workspaces: async () => (await backend()).workspaces(),
  openSample: async (sample: string, reset = false) => (await backend()).openSample(sample, reset),
  view: (world: World) => call<View>("view", { world }),
  plan: (world: World) => call<PlanData>("plan", { world }),
  apply: (ops: Op[], why = "") => call<{ created: Record<string, string> }>("apply", { ops, why }),
  /** Take back one event, or the newest change if none is given. */
  undo: (seq?: number, why = "") => call<{ seq: number | null }>("undo", { seq, why }),
  /** Take back the newest undo. */
  redo: () => call<{ seq: number | null }>("redo"),
  tryFormula: (node: string, text: string, world: World) => call<Tried>("try", { node, text, world }),
  /** What these cells would be with `set` typed in as each value in turn, with nothing changed. */
  sweep: (set: CellRef, values: number[], read: CellRef[], world: World) => call<{ at: number; cells: (Cell | null)[] }[]>("sweep", { set, values, read, world }),
  /** A numbered table's rows from one key to another, including rows nobody wrote, which are worked out from the table and never saved. */
  rows: (table: string, from: number, to: number, world: World) => call<RowCells[]>("rows", { table, from, to, world }),
  history: (node?: string) => call<EventView[]>("history", { node }),
  files: async () => (await backend()).files(),
  file: async (path: string) => (await backend()).file(path),
  proposals: () => call<Proposal[]>("proposals"),
  accept: (id: string, ops?: number[]) => call<{ seq: number }>("accept", { id, ops }),
  reject: (id: string) => call<void>("reject", { id }),
};
