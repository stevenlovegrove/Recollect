// Types mirror what recollect-server sends.

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
}

export interface Check {
  severity: "broken" | "notice";
  rule: string;
  on: string[];
  detail: string;
}

export interface View {
  workspace: { name: string; about: string | null; dir: string; systems: string[]; sample: string | null; direct_writes: boolean };
  /** Where the log is now. */
  last_seq: number | null;
  /** If it's gone back: the newest event that follows from here. */
  latest: number | null;
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
  /** The event this one follows. */
  parent: number | null;
  /** Whether it's part of what led to where the log is now. */
  here: boolean;
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

export interface Workspaces {
  current: { name: string; dir: string; sample: string | null } | null;
  samples: SampleInfo[];
  systems: { name: string; about: string }[];
  home: string;
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

/** A change to make. See crates/recollect-server/src/ops.rs for what these can say. */
export type Op =
  | { op: "add_node"; id: string; name?: string; body?: string; components?: string[]; cells?: Record<string, Written>; links?: { to: string; component: string; cells?: Record<string, Written> }[] }
  | { op: "add" | "remove"; id: string; component: string }
  | { op: "set"; id: string; cell: string; value?: unknown; formula?: string; labels?: Labels }
  | { op: "set"; from: string; to: string; cell: string; value?: unknown; formula?: string }
  | { op: "link"; from: string; to: string; component: string; cells?: Record<string, Written> }
  | { op: "unlink"; from: string; to: string; component: string }
  | { op: "delete_node"; id: string };

class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch(path, { headers: { "Content-Type": "application/json" }, ...init });
  if (!r.ok) {
    let msg = `${r.status} ${r.statusText}`;
    try {
      msg = (await r.json()).error ?? msg;
    } catch {
      /* not JSON */
    }
    throw new ApiError(msg, r.status);
  }
  return r.status === 204 ? (undefined as T) : r.json();
}

const post = <T>(path: string, body: unknown) => req<T>(path, { method: "POST", body: JSON.stringify(body) });

export const isNothingOpen = (e: unknown) => e instanceof ApiError && e.status === 409;

export const api = {
  workspaces: () => req<Workspaces>("/api/workspaces"),
  openSample: (sample: string, reset = false) => post<void>("/api/workspaces/open", { sample, reset }),
  openPath: (path: string) => post<void>("/api/workspaces/open", { path }),
  create: (path: string, name: string, systems: string[]) => post<void>("/api/workspaces/new", { path, name, systems }),
  view: (world: World) => post<View>("/api/view", { world }),
  plan: (world: World) => post<PlanData>("/api/plan", { world }),
  apply: (ops: Op[], why = "") => post<{ created: Record<string, string> }>("/api/ops", { ops, why }),
  checkout: (seq: number, why = "") => post<void>("/api/checkout", { seq, why }),
  undo: (seq: number, why = "") => post<{ seq: number }>("/api/undo", { seq, why }),
  tryFormula: (node: string, text: string, world: World) => post<Tried>("/api/try", { node, text, world }),
  /** What these cells would be with `set` typed in as each value in turn, with nothing changed. */
  sweep: (set: CellRef, values: number[], read: CellRef[], world: World) => post<{ at: number; cells: (Cell | null)[] }[]>("/api/sweep", { set, values, read, world }),
  /** A numbered table's rows from one key to another, including rows nobody wrote, which are worked out from the table and never saved. */
  rows: (table: string, from: number, to: number, world: World) => post<RowCells[]>("/api/rows", { table, from, to, world }),
  history: (node?: string) => req<EventView[]>(`/api/history${node ? `?node=${encodeURIComponent(node)}` : ""}`),
  files: () => req<FileInfo[]>("/api/files"),
  file: (path: string) => req<FileText>(`/api/file?path=${encodeURIComponent(path)}`),
  proposals: () => req<Proposal[]>("/api/proposals"),
  accept: (id: string, ops?: number[]) => post<{ seq: number }>(`/api/proposals/${encodeURIComponent(id)}/accept`, { ops }),
  reject: (id: string) => req<void>(`/api/proposals/${encodeURIComponent(id)}/reject`, { method: "POST" }),
};

export type ServerEvent = { type: "graph_changed"; by: string } | { type: "proposals_changed" } | { type: "workspace_changed" } | { type: "resync" };

/** Subscribe to server events; reconnects with backoff. Returns an unsubscribe fn. */
export function subscribe(onEvent: (e: ServerEvent) => void, onStatus: (connected: boolean) => void): () => void {
  let ws: WebSocket | null = null;
  let closed = false;
  let delay = 500;
  const connect = () => {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    ws = new WebSocket(`${proto}://${location.host}/api/events`);
    ws.onopen = () => {
      delay = 500;
      onStatus(true);
      onEvent({ type: "resync" });
    };
    ws.onmessage = (m) => onEvent(JSON.parse(m.data));
    ws.onclose = () => {
      onStatus(false);
      if (!closed) setTimeout(connect, (delay = Math.min(delay * 2, 8000)));
    };
  };
  connect();
  return () => {
    closed = true;
    ws?.close();
  };
}
