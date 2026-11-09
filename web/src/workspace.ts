import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, isNothingOpen, subscribe, type ComponentInfo, type GraphNode, type Link, type Op, type PlanData, type Proposal, type View, type Workspaces, type World } from "./api";

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
  /** Nodes whose rules ran for the last change. */
  recent: Set<string>;
  /** The same, as `node|component`. */
  recentCells: Set<string>;
  /** Choices pinned to an option, which everything on screen is narrowed by. */
  world: World;
  pin: (choice: string, option: string | null) => void;
  setWorld: (w: World) => void;
  /** Go to any point in the log. The next change follows from there. */
  checkout: (seq: number, why?: string) => Promise<void>;
  /** The kinds of link that give this graph its shape. */
  arranging: string[];
  /** Which end of a link goes on top, if it gives the graph its shape at all. */
  shape: (l: Link) => { parent: string; child: string } | null;
  /** Which end of any link comes first, if a map were laid out by it. */
  ends: (l: Link) => { parent: string; child: string };
  /** The kind of link that makes a node part of another, if this workspace has one. */
  partKind: string | undefined;
  /** A component's meaning, if a system says. */
  info: (name: string) => ComponentInfo | undefined;
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
        setRecentCells(new Set(v.ran.map((r) => `${r.node}|${r.component}`)));
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
  const arranging = useMemo(() => (view?.components ?? []).filter((c) => c.arranges).map((c) => c.name), [view?.components]);
  const reversed = useMemo(() => (view?.components ?? []).filter((c) => c.reversed).map((c) => c.name), [view?.components]);
  const partKind = arranging.includes("part_of") ? "part_of" : arranging.find((k) => !reversed.includes(k));
  const ends = useCallback((l: Link) => (reversed.includes(l.kind) ? { parent: l.from, child: l.to } : { parent: l.to, child: l.from }), [reversed]);
  const shape = useCallback((l: Link) => (arranging.includes(l.kind) ? ends(l) : null), [arranging, ends]);
  const infoMap = useMemo(() => new Map((view?.components ?? []).map((c) => [c.name, c])), [view?.components]);
  const info = useCallback((name: string) => infoMap.get(name), [infoMap]);

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

  return { view, nothingOpen, byId, plan, proposals, available, connected, error, recent, recentCells, world, pin, setWorld, checkout, arranging, shape, ends, partKind, info, refresh, apply, openSample, openPath, create };
}

/** Links touching a node, in each direction. */
/**
 * A node's links, the way the map's arrows point: a name a formula uses points into the node whose formula
 * reads it, though it's written the other way, from the reader.
 */
export function linksOf(view: View, id: string, ends: (l: Link) => { parent: string; child: string }): { outgoing: Link[]; incoming: Link[] } {
  return { outgoing: view.links.filter((l) => ends(l).child === id), incoming: view.links.filter((l) => ends(l).parent === id) };
}
