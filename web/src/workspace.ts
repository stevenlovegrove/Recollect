import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, isNothingOpen, subscribe, type CellInfo, type ComponentInfo, type Edge, type GraphNode, type Op, type PlanData, type Proposal, type View, type Workspaces, type World } from "./api";

export interface Workspace {
  view: View | null;
  /** No workspace is open, so the app offers some. */
  nothingOpen: boolean;
  byId: Map<string, GraphNode>;
  plan: PlanData;
  proposals: Proposal[];
  available: Workspaces | null;
  connected: boolean;
  error: string | null;
  /** Nodes whose cells were worked out again for the last change. */
  recent: Set<string>;
  /** The same, as `node|cell`. */
  recentCells: Set<string>;
  /** Choices pinned to an option, which everything on screen is narrowed by. */
  world: World;
  pin: (choice: string, option: string | null) => void;
  setWorld: (w: World) => void;
  /** Go to any point in the log. The next change follows from there. */
  checkout: (seq: number, why?: string) => Promise<void>;
  /** Edges by id. */
  edgeById: Map<string, Edge>;
  /** The edge components that give this graph its shape. */
  arranging: string[];
  /** Which end of an edge goes on top, if it gives the graph its shape at all. */
  shape: (e: Edge) => { parent: string; child: string } | null;
  /** Which end of any edge comes first, if a map were laid out by it. */
  ends: (e: Edge) => { parent: string; child: string };
  /** The edge component that makes a node part of another, if this workspace has one. */
  partKind: string | undefined;
  /** What a system says a component is. */
  info: (name: string) => ComponentInfo | undefined;
  /** What a system says a cell is. Nothing for one of the workspace's own cells. */
  cellInfo: (name: string) => CellInfo | undefined;
  refresh: () => Promise<void>;
  apply: (ops: Op[], why?: string) => Promise<Record<string, string>>;
  openSample: (name: string, reset?: boolean) => Promise<void>;
  openPath: (path: string) => Promise<void>;
  create: (path: string, name: string, systems: string[]) => Promise<void>;
}

const NO_PLAN: PlanData = { work: [], decisions: [] };

export function useWorkspace(): Workspace {
  const [view, setView] = useState<View | null>(null);
  const [nothingOpen, setNothingOpen] = useState(false);
  const [plan, setPlan] = useState<PlanData>(NO_PLAN);
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [available, setAvailable] = useState<Workspaces | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<Set<string>>(new Set());
  const [recentCells, setRecentCells] = useState<Set<string>>(new Set());
  const lastDir = useRef<string | null>(null);
  const [world, setWorld] = useState<World>({});
  const recentTimer = useRef<number | undefined>(undefined);
  const worldRef = useRef(world);
  worldRef.current = world;
  // Only the newest answer counts: clicking through worlds can overtake a slower reply.
  const asked = useRef(0);

  const refresh = useCallback(async () => {
    const n = ++asked.current;
    try {
      const v = await api.view(worldRef.current);
      const p = await api.plan(worldRef.current);
      if (n !== asked.current) return;
      setView(v);
      setPlan(p);
      setNothingOpen(false);
      setError(null);
      // Everything runs the first time a graph is read, which says nothing about a change.
      const opened = lastDir.current !== v.workspace.dir;
      lastDir.current = v.workspace.dir;
      if (v.ran.length && !opened) {
        setRecent(new Set(v.ran.map((r) => r.node)));
        setRecentCells(new Set(v.ran.map((r) => `${r.node}|${r.cell}`)));
        window.clearTimeout(recentTimer.current);
        recentTimer.current = window.setTimeout(() => {
          setRecent(new Set());
          setRecentCells(new Set());
        }, 2500);
      }
    } catch (e) {
      if (n !== asked.current) return;
      if (isNothingOpen(e)) {
        setView(null);
        setNothingOpen(true);
        setError(null);
      } else setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const loadProposals = useCallback(async () => {
    try {
      setProposals(await api.proposals());
    } catch (e) {
      if (isNothingOpen(e)) setProposals([]);
    }
  }, []);

  const loadAvailable = useCallback(async () => setAvailable(await api.workspaces()), []);

  useEffect(() => {
    return subscribe((ev) => {
      if (ev.type === "graph_changed") {
        refresh();
        loadProposals();
      } else if (ev.type === "proposals_changed") {
        loadProposals();
        refresh();
      } else if (ev.type === "workspace_changed") {
        setWorld({});
        worldRef.current = {};
        refresh();
        loadProposals();
        loadAvailable();
      } else {
        refresh();
        loadProposals();
        loadAvailable();
      }
    }, setConnected);
  }, [refresh, loadProposals, loadAvailable]);

  // A different world is only a different question.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) first.current = false;
    else refresh();
  }, [world, refresh]);

  // Rows nobody wrote that something reads can be looked up too, though they're not in `nodes`.
  const byId = useMemo(() => new Map([...(view?.nodes ?? []), ...(view?.unwritten ?? [])].map((n) => [n.id, n])), [view]);
  const edgeById = useMemo(() => new Map((view?.edges ?? []).map((e) => [e.id, e])), [view?.edges]);
  const arranging = useMemo(() => (view?.components ?? []).filter((c) => c.arranges).map((c) => c.name), [view?.components]);
  const reversed = useMemo(() => (view?.components ?? []).filter((c) => c.reversed).map((c) => c.name), [view?.components]);
  const partKind = arranging.includes("part_of") ? "part_of" : arranging.find((k) => !reversed.includes(k));
  // An edge goes the way its first component that gives a graph its shape says, so a name a formula uses
  // points into what reads it.
  const ends = useCallback(
    (e: Edge) => {
      const lead = e.components.find((c) => arranging.includes(c)) ?? e.components[0];
      return reversed.includes(lead) ? { parent: e.from, child: e.to } : { parent: e.to, child: e.from };
    },
    [arranging, reversed],
  );
  const shape = useCallback((e: Edge) => (e.components.some((c) => arranging.includes(c)) ? ends(e) : null), [arranging, ends]);
  const infoMap = useMemo(() => new Map((view?.components ?? []).map((c) => [c.name, c])), [view?.components]);
  const info = useCallback((name: string) => infoMap.get(name), [infoMap]);
  const cellMap = useMemo(() => new Map((view?.cells ?? []).map((c) => [c.name, c])), [view?.cells]);
  const cellInfo = useCallback((name: string) => cellMap.get(name), [cellMap]);

  const pin = useCallback((choice: string, option: string | null) => {
    setWorld((w) => {
      const next = { ...w };
      if (option === null) delete next[choice];
      else next[choice] = option;
      return next;
    });
  }, []);

  const apply = useCallback(
    async (ops: Op[], why = "") => {
      const done = await api.apply(ops, why);
      await refresh();
      return done.created;
    },
    [refresh],
  );

  const checkout = useCallback(
    async (seq: number, why = "") => {
      await api.checkout(seq, why);
      await refresh();
    },
    [refresh],
  );

  const openSample = useCallback(async (name: string, reset = false) => {
    await api.openSample(name, reset);
  }, []);
  const openPath = useCallback(async (path: string) => {
    await api.openPath(path);
  }, []);
  const create = useCallback(async (path: string, name: string, systems: string[]) => {
    await api.create(path, name, systems);
  }, []);

  return { view, nothingOpen, byId, edgeById, plan, proposals, available, connected, error, recent, recentCells, world, pin, setWorld, checkout, arranging, shape, ends, partKind, info, cellInfo, refresh, apply, openSample, openPath, create };
}

/**
 * A node's edges, the way the map's arrows point: a name a formula uses points into the node whose formula
 * reads it, though it's written the other way, from the reader.
 */
export function edgesOf(view: View, id: string, ends: (e: Edge) => { parent: string; child: string }): { outgoing: Edge[]; incoming: Edge[] } {
  return { outgoing: view.edges.filter((e) => ends(e).child === id), incoming: view.edges.filter((e) => ends(e).parent === id) };
}

/** The edge from one node to another, if there is one. */
export function edgeBetween(view: View, from: string, to: string): Edge | undefined {
  return view.edges.find((e) => e.from === from && e.to === to);
}

/** The nodes a node's formula calls each name, from its label edges. */
export function labelsOf(view: View, id: string): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const e of view.edges) {
    if (e.from !== id || !e.components.includes("label")) continue;
    const name = e.cells.called?.value;
    if (typeof name === "string") (out[name] ??= []).push(e.to);
  }
  return out;
}
