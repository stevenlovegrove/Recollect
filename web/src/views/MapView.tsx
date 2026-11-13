import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  ControlButton,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  useUpdateNodeInternals,
  type Edge as RFEdge,
  type Node as RFNode,
  type NodeProps,
} from "@xyflow/react";
import ELK from "elkjs/lib/elk.bundled.js";
import type { GraphNode } from "../api";
import type { Workspace } from "../workspace";
import { badgesOf, compColor, dominant, isDropped, isNote, label as labelOf, nodeColor } from "../theme";
import { Brackets, CompBadges, beliefText, edgeKey, edgeText, parseEdgeKey, reading } from "../bits";
import { type SummaryLine, summaryOf } from "../summary";
import { DdItem, Dropdown } from "../Dropdown";
import { Key, isTyping } from "../Shortcuts";
import type { CtxTarget } from "../ContextMenu";

export interface ViewProps {
  ws: Workspace;
  selected: string | null;
  onSelect: (id: string | null) => void;
}

const elk = new ELK();
const IS_MAC = /Mac/.test(navigator.platform);
const W = 230;
const CARD_H = 70;
type Filter = string;

function nodeHeight(n: GraphNode) {
  return 50 + Math.ceil(n.name.length / 29) * 17;
}

/** How much taller a card is for the options drawn on it, which wrap onto as many rows as they need. */
function optionsHeight(options: { name: string; chance: number | null }[]) {
  let [rows, used] = [options.length ? 1 : 0, 0];
  for (const o of options) {
    const w = 14 + 5.5 * (o.name.length + (o.chance === null ? 0 : 4)) + 4;
    if (used + w > W - 22 && used > 0) [rows, used] = [rows + 1, 0];
    used += w;
  }
  return rows ? rows * 22 + 22 : 0;
}

/** Which way a map reads: what comes first goes at the top, or on the left. */
type Direction = "down" | "right";

/** Marks a row nobody wrote, wherever one is shown. */
export const UNWRITTEN = "◇";

type CardData = {
  ws: Workspace;
  node: GraphNode;
  direction: Direction;
  selected: boolean;
  recent: boolean;
  dim: boolean;
  entering: boolean;
  leaving: boolean;
  linking: boolean;
  /** Not needed in any world still in play. */
  out: boolean;
  lines: SummaryLine[];
  /** The names of what it's a kind of. */
  kinds: string[];
  title: string;
  /** For a question whose options are drawn on it rather than as cards: each, to pin. */
  options: { choice: string; id: string; name: string; chance: number | null; picked: boolean; possible: boolean }[];
  /** The names of its parts that aren't drawn as cards. */
  holds: string[];
};

function Card({ data }: NodeProps<RFNode<CardData>>) {
  const { node, direction, selected, recent, dim, entering, leaving, linking, out, kinds, options, holds } = data;
  // The options say how each is believed and what it could land on, so those lines would say it twice.
  const lines = options.length ? data.lines.filter((l) => l.name !== "belief" && l.name !== "chosen") : data.lines;
  return (
    <div
      className={["card", node.unwritten && "unwritten", isDropped(node) && "dead", out && "out", selected && "selected", recent && "recent", dim && "dim", entering && "entering", leaving && "leaving", linking && "linking"].filter(Boolean).join(" ")}
      style={{ borderLeftColor: nodeColor(node), width: W }}
    >
      {/* Links point child → parent, and parents come first: leave from that side. */}
      <Handle type="source" position={direction === "down" ? Position.Top : Position.Left} />
      <div className="card-top">
        <CompBadges ws={data.ws} n={node} />
        {kinds.map((k) => (
          <span key={k} className="cbadge" title="What kind of thing it is">
            {k}
          </span>
        ))}
        {isNote(node) && !kinds.length && <span className="muted small">note</span>}
      </div>
      <div className="card-title" title={node.unwritten ? `${node.name}: nobody wrote this row. It's worked out from its table, and isn't saved.` : node.name}>
        {node.unwritten && <span className="unwritten-mark">{UNWRITTEN} </span>}
        {data.title}
      </div>
      {lines.map((l) => (
        <div key={l.name} className={`card-foot ${l.trivial ? "trivial" : ""}`}>
          <span className="card-foot-k" title={`${l.about ? `${l.about}. ` : ""}${labelOf(l.name)}: ${l.text.length > 400 ? `${l.text.slice(0, 400)}…` : l.text}`}>{labelOf(l.name)}</span> <span className="card-foot-v"><Brackets text={l.text} /></span>
        </div>
      ))}
      {options.length > 0 && (
        <div className="card-opts nodrag nopan">
          {options.map((o) => (
            <button
              key={o.id}
              className={`card-opt ${o.picked ? "picked" : ""} ${o.possible ? "" : "gone"}`}
              title={o.picked ? `Pinned to ${o.name}. Click to unpin` : `See everything as if it were ${o.name}`}
              onClick={(e) => {
                e.stopPropagation();
                data.ws.pin(o.choice, o.picked ? null : o.id);
              }}
            >
              {o.name}
              {o.chance !== null && <span className="muted"> {beliefText(o.chance)}</span>}
            </button>
          ))}
        </div>
      )}
      {holds.length > 0 && (
        <div className="card-foot">
          <span className="card-foot-k">Has</span> <span className="card-foot-v">{holds.join(", ")}</span>
        </div>
      )}
      <Handle type="target" position={direction === "down" ? Position.Bottom : Position.Right} />
    </div>
  );
}

const nodeTypes = { card: Card };

type XY = { x: number; y: number };

/** Where the minimap sits: an offset from whichever corner it was dropped nearest. */
type MiniPref = { open: boolean; h: "left" | "right"; x: number; v: "top" | "bottom"; y: number };
const MINI_KEY = "recollect.minimap";
const MINI_W = 200;
const MINI_H = 168; // map plus its drag handle
const MINI_DEFAULT: MiniPref = { open: true, h: "right", x: 16, v: "bottom", y: 16 };

function loadMini(): MiniPref {
  try {
    return { ...MINI_DEFAULT, ...JSON.parse(localStorage.getItem(MINI_KEY) ?? "{}") };
  } catch {
    return MINI_DEFAULT;
  }
}

function saveMini(p: MiniPref) {
  try {
    localStorage.setItem(MINI_KEY, JSON.stringify(p));
  } catch {
    /* Storage unavailable: the preference just won't persist. */
  }
}

/** The minimap in a floating frame: drag the top strip to move it, × to close it. */
function FloatingMiniMap({ pref, onChange, canvas }: { pref: MiniPref; onChange: (p: MiniPref) => void; canvas: React.RefObject<HTMLDivElement | null> }) {
  const [drag, setDrag] = useState<XY | null>(null);
  const startDrag = (e: React.PointerEvent) => {
    const frame = (e.currentTarget as HTMLElement).parentElement!.getBoundingClientRect();
    const bounds = canvas.current?.getBoundingClientRect();
    if (!bounds || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const grabX = e.clientX - frame.left;
    const grabY = e.clientY - frame.top;
    const place = (ev: PointerEvent): XY => ({
      x: Math.max(0, Math.min(bounds.width - frame.width, ev.clientX - bounds.left - grabX)),
      y: Math.max(0, Math.min(bounds.height - frame.height, ev.clientY - bounds.top - grabY)),
    });
    const onMove = (ev: PointerEvent) => setDrag(place(ev));
    const onUp = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      const at = place(ev);
      const right = bounds.width - frame.width - at.x;
      const bottom = bounds.height - frame.height - at.y;
      setDrag(null);
      onChange({
        ...pref,
        ...(at.x <= right ? { h: "left", x: Math.round(at.x) } : { h: "right", x: Math.round(right) }),
        ...(at.y <= bottom ? { v: "top", y: Math.round(at.y) } : { v: "bottom", y: Math.round(bottom) }),
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };
  // The min() keeps a saved offset on screen if the canvas has since become smaller.
  const style: React.CSSProperties = drag
    ? { left: drag.x, top: drag.y }
    : { [pref.h]: `min(${pref.x}px, calc(100% - ${MINI_W}px))`, [pref.v]: `min(${pref.y}px, calc(100% - ${MINI_H}px))` };
  return (
    <div className={`mini-frame ${drag ? "dragging" : ""}`} style={style}>
      <div className="mini-handle nodrag nopan" onPointerDown={startDrag} title="Drag to move">
        <span className="mini-grip">⠿</span>
        <button
          className="icon small"
          title="Close the minimap (m)"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => onChange({ ...pref, open: false })}
        >
          ×
        </button>
      </div>
      <MiniMap
        pannable
        zoomable
        ariaLabel="Overview: drag to move the view"
        nodeColor={(n) => compColor(dominant((n.data as CardData).node))}
        nodeBorderRadius={6}
        maskColor="var(--minimap-mask)"
      />
    </div>
  );
}
const ANIM_MS = 600;
const FIT_MS = 450;
const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/** How this map is laid out, per workspace and per browser. It's fine if it can't be remembered. */
type LayoutPref = { direction: Direction; layoutBy: Record<string, boolean>; /** Draw an option of several questions on each of them, not as a card. */ fold?: boolean };

function loadLayoutPref(dir: string): LayoutPref {
  try {
    return { direction: "down", layoutBy: {}, ...JSON.parse(localStorage.getItem(`recollect.layout:${dir}`) ?? "{}") };
  } catch {
    return { direction: "down", layoutBy: {} };
  }
}

function saveLayoutPref(dir: string, pref: LayoutPref) {
  try {
    localStorage.setItem(`recollect.layout:${dir}`, JSON.stringify(pref));
  } catch {
    /* it just won't be remembered */
  }
}

const LABEL_REL = "label:";

/**
 * A line the map draws: an edge, by the component that leads it (the first that gives the graph its shape,
 * and otherwise its first), or a row a node reads by where it is. `value` is what a formula calls the end.
 */
type MapLink = { from: string; to: string; kind: string; value: unknown; key: string; holds?: string };

/** What the Links menu filters a link by. */
const relOf = (e: { kind: string; value: unknown }) => (e.kind === "label" ? `${LABEL_REL}${String(e.value)}` : e.kind);

/** Whether a kind of link shapes the layout, inside a row whose own tick says whether it's shown. */
function LayoutTick({ on, name, onChange }: { on: boolean; name: string; onChange: (on: boolean) => void }) {
  return (
    <input
      type="checkbox"
      className="dd-layout"
      checked={on}
      aria-label={`Lay out by ${name}`}
      title={`Lay out by ${name}`}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => {
        e.preventDefault();
        onChange(e.target.checked);
      }}
    />
  );
}

type LinkDrag = { from: string; start: XY; end: XY; over: string | null };
type LinkDrop = { from: string; to: string; at: XY };
let lastKind = "";

/** Shown where a link was dropped: pick its kind, and flip its direction if needed. */
function LinkMenu({ ws, drop, onSwap, onDone }: { ws: Workspace; drop: LinkDrop; onSwap: () => void; onDone: (kind?: string) => void }) {
  // A label needs a name to mean anything, so it's made from a formula, in the inspector.
  const kinds = ws.view!.components.filter((c) => c.place === "edge" && c.name !== "label");
  const exists = (kind: string) => ws.view!.edges.some((e) => e.from === drop.from && e.to === drop.to && e.components.includes(kind));
  const first = kinds.findIndex((r) => r.name === lastKind && !exists(r.name));
  const [cursor, setCursor] = useState(first >= 0 ? first : Math.max(0, kinds.findIndex((r) => !exists(r.name))));
  const [err, setErr] = useState<string | null>(null);
  const title = (id: string) => ws.byId.get(id)?.name ?? id;

  const create = async (kind: string) => {
    if (exists(kind)) return;
    try {
      await ws.apply([{ op: "link", from: drop.from, to: drop.to, component: kind }]);
      lastKind = kind;
      onDone(kind);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    // Capture, so tab numbers, Esc and arrows act on the menu while it's open.
    const onKey = (e: KeyboardEvent) => {
      const digit = Number(e.key);
      if (e.key === "Escape") onDone();
      else if (e.key === "ArrowDown") setCursor((c) => (c + 1) % kinds.length);
      else if (e.key === "ArrowUp") setCursor((c) => (c + kinds.length - 1) % kinds.length);
      else if (e.key === "Enter") create(kinds[cursor].name);
      else if (e.key === "s") onSwap();
      else if (digit >= 1 && digit <= kinds.length) create(kinds[digit - 1].name);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    const onDown = (e: PointerEvent) => {
      if (!(e.target as Element).closest?.(".link-menu")) onDone();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  });

  return (
    <div className="link-menu" style={{ left: drop.at.x, top: drop.at.y }}>
      <div className="link-ends">
        <div className="link-end" title={title(drop.from)}>
          {title(drop.from)}
        </div>
        <button className="link-swap" title="Swap direction (s)" onClick={onSwap}>
          ↓ ⇅
        </button>
        <div className="link-end" title={title(drop.to)}>
          {title(drop.to)}
        </div>
      </div>
      {kinds.map((r, i) => (
        <button
          key={r.name}
          disabled={exists(r.name)}
          className={`link-rel ${i === cursor ? "on" : ""}`}
          title={r.about}
          onMouseEnter={() => setCursor(i)}
          onClick={() => create(r.name)}
        >
          <kbd>{i + 1}</kbd>
          <span className={`dd-swatch rel-${r.name}`} />
          <span>{labelOf(r.name)}</span>
          {exists(r.name) && <span className="muted small">exists</span>}
        </button>
      ))}
      {err && <div className="error">{err}</div>}
    </div>
  );
}

function MapInner({ ws, selected, onSelect, active, onContext }: ViewProps & { active: boolean; onContext: (t: CtxTarget) => void }) {
  const view = ws.view!;
  // A row nobody wrote is on the map when something written reads it, and only then.
  const g = useMemo(() => ({ ...view, nodes: [...view.nodes, ...view.unwritten] }), [view]);
  // Rows of related tables with one key are one thing, so they're one card: the part made first stands for
  // the rest, and their links are its.
  const standsFor = useMemo(() => {
    const m = new Map<string, string>();
    for (const n of g.nodes) if (n.parts?.length) m.set(n.id, [n.id, ...n.parts].sort()[0]);
    return m;
  }, [g.nodes]);
  const rep = (id: string) => standsFor.get(id) ?? id;
  // What the map draws: links between cards, and the rows a node reads by where they are, as `prev` or
  // `Year[3]` do, which no link says. Each keeps the key of the link it came from.
  const links = useMemo(() => {
    const at = (id: string) => standsFor.get(id) ?? id;
    const out: MapLink[] = [];
    for (const e of g.edges) {
      if (at(e.from) === at(e.to)) continue;
      const kind = e.components.find((c) => ws.arranging.includes(c)) ?? e.components[0];
      out.push({ from: at(e.from), to: at(e.to), kind, value: e.cells.called?.value, key: edgeKey(e.id), holds: edgeText(e) });
    }
    const byId = new Map(g.nodes.map((n) => [n.id, n]));
    for (const n of g.nodes)
      for (const to of n.reads ?? []) {
        const [a, b] = [n.row, byId.get(to)?.row];
        // The row before or after is called that, and anything further by its address.
        const by = a && b && a.table === b.table && typeof a.key === "number" && typeof b.key === "number" ? b.key - a.key : null;
        const value = by === -1 ? "prev" : by === 1 ? "next" : (byId.get(to)?.name ?? "");
        if (byId.has(to) && at(n.id) !== at(to)) out.push({ from: at(n.id), to: at(to), kind: "label", value, key: `reads:${n.id}|${to}` });
      }
    return out;
  }, [g.nodes, g.edges, standsFor, ws.arranging]);
  const [hiddenFacts, setHiddenFacts] = useState<Set<Filter>>(new Set());
  const [hiddenRels, setHiddenRels] = useState<Set<string>>(new Set());
  const [layoutPref, setLayoutPrefState] = useState<LayoutPref>(() => loadLayoutPref(g.workspace.id));
  const setLayoutPref = (next: LayoutPref) => {
    setLayoutPrefState(next);
    saveLayoutPref(g.workspace.id, next);
    refit.current = true;
  };
  const refit = useRef(false);
  // An option of several questions, with nothing to say but what it's an option or a part of, would be a
  // card with a line to each of them, and the lines would be most of the map. It's drawn on each instead.
  const folded = useMemo(() => {
    const out = new Set<string>();
    if (layoutPref.fold === false) return out;
    const into = new Set(g.edges.map((e) => e.to));
    const from = new Map<string, typeof g.edges>();
    for (const e of g.edges) from.set(e.from, [...(from.get(e.from) ?? []), e]);
    const asked = new Set(g.worlds.vars.map((v) => v.id));
    for (const [id, edges] of from) {
      const plain = edges.every((e) => e.components.every((c) => c === "alternative" || c === "part_of"));
      if (plain && !into.has(id) && !asked.has(id) && edges.filter((e) => e.components.includes("alternative")).length > 1) out.add(id);
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [g.edges, g.worlds.vars, layoutPref.fold]);
  // The options each question has drawn on it, where any of them isn't a card.
  const cardOptions = useMemo(() => {
    const out = new Map<string, CardData["options"]>();
    const short = (o: string, name: string) => String(ws.byId.get(o)?.row?.key ?? name);
    for (const asked of g.worlds.vars)
      if (asked.options.some((o) => folded.has(o.id)))
        out.set(asked.id, asked.options.map((o) => ({ choice: asked.id, id: o.id, name: short(o.id, o.name), chance: asked.uncertain ? o.chance : null, picked: o.picked, possible: o.possible })));
    return out;
  }, [g.worlds.vars, folded, ws.byId]);
  const cardHeight = (n: GraphNode) => nodeHeight(n) + optionsHeight(cardOptions.get(n.id) ?? []);
  const updateNodeInternals = useUpdateNodeInternals();
  const [showDead, setShowDead] = useState(false);
  const [focus, setFocus] = useState(0);
  const [query, setQuery] = useState("");
  // On a phone the toolbar is a bubble over the map, shown when wanted.
  const [opts, setOpts] = useState(false);
  // Laid-out node list (ids + sizes) and the positions currently on screen (animated).
  const [layout, setLayout] = useState<{ ids: string[]; target: Map<string, XY> }>({ ids: [], target: new Map() });
  const [pos, setPos] = useState<Map<string, XY>>(new Map());
  const posRef = useRef<Map<string, XY>>(new Map());
  const entering = useRef<Set<string>>(new Set());
  const leaving = useRef<Map<string, XY>>(new Map());
  const anim = useRef(0);
  const { fitBounds, fitView, zoomTo, getZoom, getViewport, setViewport } = useReactFlow();
  const searchRef = useRef<HTMLInputElement>(null);
  const stepMatch = useRef<(by: number) => void>(() => {});
  const [mini, setMiniState] = useState<MiniPref>(loadMini);
  const setMini = (p: MiniPref) => {
    setMiniState(p);
    saveMini(p);
  };
  const canvas = useRef<HTMLDivElement>(null);
  const [linkDrag, setLinkDrag] = useState<LinkDrag | null>(null);
  const [linkDrop, setLinkDrop] = useState<LinkDrop | null>(null);
  const selectedNode = selected && ws.byId.has(selected) ? selected : null;
  const selectedRef = useRef(selectedNode);
  selectedRef.current = selectedNode;

  const activeRef = useRef(active);
  activeRef.current = active;

  // Pan just enough to bring a card on screen (never zooms, never moves if it's already there).
  // Not while hidden: selecting things on other tabs mustn't lose your place on the map.
  const ensureVisible = (id: string | null) => {
    if (!activeRef.current) return;
    const p = id && posRef.current.get(id);
    const rect = canvas.current?.getBoundingClientRect();
    if (!p || !rect) return;
    const v = getViewport();
    const margin = 40;
    const left = p.x * v.zoom + v.x;
    const right = left + W * v.zoom;
    const top = p.y * v.zoom + v.y;
    const bottom = top + CARD_H * v.zoom;
    const dx = left < margin ? margin - left : right > rect.width - margin ? rect.width - margin - right : 0;
    const dy = top < margin ? margin - top : bottom > rect.height - margin ? rect.height - margin - bottom : 0;
    if (dx || dy) setViewport({ x: v.x + dx, y: v.y + dy, zoom: v.zoom }, { duration: 250 });
  };
  useEffect(() => ensureVisible(selectedNode), [selectedNode]); // eslint-disable-line react-hooks/exhaustive-deps

  // Centre a card in the canvas as it is right now, measured directly: when the inspector has
  // only just opened, React Flow's own width is still the wider one from before.
  const centreOn = (id: string | null, zoom?: number) => {
    const p = id && posRef.current.get(id);
    const node = id ? ws.byId.get(id) : undefined;
    const rect = canvas.current?.getBoundingClientRect();
    if (!p || !node || !rect) return;
    const k = zoom ?? getViewport().zoom;
    setViewport(
      {
        x: rect.width / 2 - (p.x + W / 2) * k,
        y: rect.height / 2 - (p.y + nodeHeight(node) / 2) * k,
        zoom: k,
      },
      { duration: FIT_MS },
    );
  };

  // Arrow keys: go to the nearest card in that direction, preferring ones straight ahead.
  const moveSelection = (dx: number, dy: number) => {
    const centre = (p: XY) => ({ x: p.x + W / 2, y: p.y + CARD_H / 2 });
    const cur = selectedNode ? posRef.current.get(selectedNode) : undefined;
    let best: string | null = null;
    let bestScore = Infinity;
    if (!cur) {
      // Nothing selected: start from whatever is nearest the middle of the view.
      const rect = canvas.current?.getBoundingClientRect();
      const v = getViewport();
      if (!rect) return;
      const cx = (rect.width / 2 - v.x) / v.zoom;
      const cy = (rect.height / 2 - v.y) / v.zoom;
      for (const [id, p] of posRef.current) {
        const c = centre(p);
        const score = (c.x - cx) ** 2 + (c.y - cy) ** 2;
        if (score < bestScore) [best, bestScore] = [id, score];
      }
    } else {
      const from = centre(cur);
      for (const [id, p] of posRef.current) {
        if (id === selectedNode) continue;
        const c = centre(p);
        const along = (c.x - from.x) * dx + (c.y - from.y) * dy;
        if (along < 1) continue;
        const across = Math.abs((c.x - from.x) * dy) + Math.abs((c.y - from.y) * dx);
        // Up and down follow the hierarchy when they can: a linked card beats a closer stranger.
        const linked = dy !== 0 && layoutLinks.some((e) => (e.from === selectedNode && e.to === id) || (e.to === selectedNode && e.from === id));
        const score = (along + 2.5 * across) * (linked ? 0.25 : 1);
        if (score < bestScore) [best, bestScore] = [id, score];
      }
    }
    if (best) onSelect(best);
  };

  // Trackpad-first pan/zoom, like Figma/Freeform: two-finger scroll pans in 2D, pinch zooms
  // around the cursor (browsers report pinch as ctrl+wheel), ⌘+wheel zooms for mouse users.
  // macOS delivers wheel events at ~60 Hz with uneven timing; applying each one immediately
  // (as React Flow does) gives uneven steps on a 120 Hz display. Instead we accumulate deltas
  // and apply them once per animation frame, as native scroll views do.
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    let dx = 0;
    let dy = 0;
    let zoomBy = 1;
    let anchor = { x: 0, y: 0 };
    let frame = 0;
    const flush = () => {
      frame = 0;
      let { x, y, zoom } = getViewport();
      if (zoomBy !== 1) {
        const k = Math.min(2.5, Math.max(0.1, zoom * zoomBy));
        x = anchor.x - ((anchor.x - x) * k) / zoom;
        y = anchor.y - ((anchor.y - y) * k) / zoom;
        zoom = k;
      }
      setViewport({ x: x - dx, y: y - dy, zoom });
      dx = dy = 0;
      zoomBy = 1;
    };
    const onWheel = (e: WheelEvent) => {
      if ((e.target as Element).closest?.(".nowheel")) return;
      e.preventDefault();
      e.stopPropagation();
      const unit = e.deltaMode === 1 ? 20 : e.deltaMode === 2 ? 400 : 1;
      if (e.ctrlKey || e.metaKey) {
        // Same curve as d3-zoom: pinch deltas are small and frequent, ⌘+wheel notches large.
        zoomBy *= Math.pow(2, -e.deltaY * unit * 0.002 * (e.ctrlKey ? 10 : 1));
        const r = el.getBoundingClientRect();
        anchor = { x: e.clientX - r.left, y: e.clientY - r.top };
      } else {
        dx += e.deltaX * unit;
        dy += e.deltaY * unit;
      }
      if (!frame) frame = requestAnimationFrame(flush);
    };
    // Capture phase, non-passive: we own the wheel before React Flow's d3 handler sees it.
    el.addEventListener("wheel", onWheel, { capture: true, passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel, { capture: true });
      cancelAnimationFrame(frame);
    };
  }, [getViewport, setViewport, g.nodes.length === 0]);

  // ⇧-drag from one card to another draws a directed edge. Capture phase, so React Flow's
  // pan and selection never see the gesture.
  useEffect(() => {
    const el = canvas.current;
    if (!el || !active) return;
    const cardAt = (t: EventTarget | null) => (t instanceof Element ? t.closest<HTMLElement>(".react-flow__node") : null);
    const swallow = (e: MouseEvent) => {
      if (e.shiftKey && e.button === 0 && cardAt(e.target)) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    const onDown = (e: PointerEvent) => {
      const card = cardAt(e.target);
      if (!e.shiftKey || e.button !== 0 || !card?.dataset.id) return;
      e.preventDefault();
      e.stopPropagation();
      const from = card.dataset.id;
      const b = el.getBoundingClientRect();
      const centre = (c: Element): XY => {
        const r = c.getBoundingClientRect();
        return { x: r.left + r.width / 2 - b.left, y: r.top + r.height / 2 - b.top };
      };
      const start = centre(card);
      const read = (ev: PointerEvent): LinkDrag => {
        const hit = cardAt(document.elementFromPoint(ev.clientX, ev.clientY));
        const over = hit?.dataset.id && hit.dataset.id !== from ? hit : null;
        return { from, start, end: over ? centre(over) : { x: ev.clientX - b.left, y: ev.clientY - b.top }, over: over?.dataset.id ?? null };
      };
      const stop = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("keydown", onKey, true);
        setLinkDrag(null);
      };
      const onMove = (ev: PointerEvent) => setLinkDrag(read(ev));
      const onUp = (ev: PointerEvent) => {
        const last = read(ev);
        stop();
        if (last.over) {
          const at = { x: Math.min(ev.clientX - b.left, b.width - 270), y: Math.min(ev.clientY - b.top, b.height - 350) };
          setLinkDrop({ from, to: last.over, at: { x: Math.max(8, at.x), y: Math.max(8, at.y) } });
        }
      };
      const onKey = (ev: KeyboardEvent) => {
        if (ev.key === "Escape") {
          ev.stopPropagation();
          stop();
        }
      };
      setLinkDrop(null);
      setLinkDrag(read(e));
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("keydown", onKey, true);
    };
    el.addEventListener("pointerdown", onDown, true);
    el.addEventListener("mousedown", swallow, true);
    return () => {
      el.removeEventListener("pointerdown", onDown, true);
      el.removeEventListener("mousedown", swallow, true);
    };
  }, [active, g.nodes.length === 0]);

  // Keyboard: F fits everything, 1 resets to 100%, Esc clears the selection.
  // ⌘0/⌘±/⌘-scroll stay with the browser so the page itself still zooms as expected.
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      // ⌘G / ⇧⌘G: the Mac's find next / previous, from anywhere on the map.
      if (e.metaKey && (e.key === "g" || e.key === "G") && !document.querySelector(".modal")) {
        e.preventDefault();
        stepMatch.current(e.shiftKey ? -1 : 1);
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || document.querySelector(".modal, .link-menu, .ctx-menu")) return;
      if (e.key === "ArrowLeft") moveSelection(-1, 0);
      else if (e.key === "ArrowRight") moveSelection(1, 0);
      else if (e.key === "ArrowUp") moveSelection(0, -1);
      else if (e.key === "ArrowDown") moveSelection(0, 1);
      else if (e.key === "/") searchRef.current?.focus();
      else if (e.key === "m") setMini({ ...loadMini(), open: !loadMini().open });
      else if (e.key === "z") {
        if (!selectedNode) return;
        centreOn(selectedNode, 1);
      } else if (e.key === "f" || e.key === "F") fitView({ padding: 0.08, duration: FIT_MS });
      else if (e.key === "0") zoomTo(1, { duration: 350 });
      else if (e.key === "=" || e.key === "+") zoomTo(Math.min(2.5, getZoom() * 1.25), { duration: 250 });
      else if (e.key === "-") zoomTo(Math.max(0.1, getZoom() / 1.25), { duration: 250 });
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitView, zoomTo, getZoom, onSelect, selectedNode, active]);

  const factsFor = (n: GraphNode): Filter[] => {
    const f = badgesOf(n);
    return f.length ? f : isNote(n) ? ["none"] : ["own"];
  };
  const present = useMemo(() => {
    const p = new Map<Filter, number>();
    for (const n of g.nodes) for (const c of factsFor(n)) p.set(c, (p.get(c) ?? 0) + 1);
    return [...p].sort((a, b) => a[0].localeCompare(b[0]));
  }, [g.nodes]);
  // What the Links menu offers. A label's name says more than its kind does, since in a sheet or a
  // puzzle every link is a label, so each name can be shown or hidden.
  const rels = useMemo(() => {
    const counts = new Map<string, number>();
    for (const e of links) counts.set(relOf(e), (counts.get(relOf(e)) ?? 0) + 1);
    return [...counts].sort((a, b) => a[0].localeCompare(b[0]));
  }, [links]);
  // A system says which links shape a map, and you can say otherwise.
  const shapes = useCallback(
    (rel: string) => layoutPref.layoutBy[rel] ?? (rel.startsWith(LABEL_REL) ? ws.arranging.includes("label") : !!(ws.arranging.includes(rel) || ws.info(rel)?.orders)),
    [layoutPref, ws.arranging, ws.info],
  );
  const setShapes = (rels: string[], on: boolean) => setLayoutPref({ ...layoutPref, layoutBy: { ...layoutPref.layoutBy, ...Object.fromEntries(rels.map((r) => [r, on])) } });
  const labelRels = rels.filter(([r]) => r.startsWith(LABEL_REL));
  const kindRels = rels.filter(([r]) => !r.startsWith(LABEL_REL));

  // A node is shown if any of its facts (or "none") is enabled.
  const visible = useMemo(() => {
    let ids = new Set(g.nodes.filter((n) => !folded.has(n.id) && (!standsFor.has(n.id) || standsFor.get(n.id) === n.id)).filter((n) => (showDead || !isDropped(n)) && factsFor(n).some((c) => !hiddenFacts.has(c))).map((n) => n.id));
    if (focus > 0 && selectedNode && ids.has(selectedNode)) {
      const near = new Set([selectedNode]);
      let frontier = [selectedNode];
      for (let d = 0; d < focus; d++) {
        const next: string[] = [];
        for (const e of links)
          for (const [a, b] of [
            [e.from, e.to],
            [e.to, e.from],
          ])
            if (frontier.includes(a) && ids.has(b) && !near.has(b)) {
              near.add(b);
              next.push(b);
            }
        frontier = next;
      }
      ids = near;
    }
    return ids;
  }, [g.nodes, links, standsFor, folded, hiddenFacts, showDead, focus, selectedNode]);

  const q = query.trim().toLowerCase();
  const matches = (n: GraphNode) => !q || `${n.id} ${n.name}`.toLowerCase().includes(q);

  // Search results among the cards on the map, in reading order.
  const found = useMemo(() => {
    if (!q) return [];
    const at = (id: string) => layout.target.get(id) ?? { x: 0, y: 0 };
    return layout.ids
      .filter((id) => {
        const n = ws.byId.get(id);
        return n && matches(n);
      })
      .sort((a, b) => at(a).y - at(b).y || at(a).x - at(b).x);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, layout, ws.byId]);
  const foundAt = selectedNode ? found.indexOf(selectedNode) : -1;
  stepMatch.current = (by) => {
    if (!found.length) return;
    const from = foundAt >= 0 ? foundAt + by : by > 0 ? 0 : found.length - 1;
    onSelect(found[(from + found.length) % found.length]);
  };

  // Esc on the canvas clears an active search before it deselects. Runs ahead of the app's
  // handler; anything more immediate (a form, a menu, the shortcut sheet, a field) goes first.
  useEffect(() => {
    if (!q || !active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || isTyping(e.target) || document.querySelector(".modal, .dd-pop, .shortcut-sheet.sticky")) return;
      e.stopPropagation();
      setQuery("");
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [q, active]);

  // Layout depends only on visible nodes and the links that give the graph its shape, never on
  // what's filtered or which world is being looked at, so nothing jumps when you decide.
  const layoutEdges = useMemo(() => {
    const seen = new Set<string>();
    // A link's other kinds decide for it, so a label on a `part_of` doesn't pull it the other way.
    const others = new Set(links.filter((e) => e.kind !== "label" && shapes(relOf(e))).map((e) => `${e.from}|${e.to}`));
    // A formula taken from a kind reads what the kind labels, so that goes beneath the node it runs for too.
    const taken = g.nodes.flatMap((n) =>
      [...new Set(Object.values(n.cells).filter((c) => c.source === "kind" && c.kind && c.formula).map((c) => c.kind!))].flatMap((k) =>
        links.filter((l) => l.from === k && l.kind === "label").map((l) => ({ ...l, from: rep(n.id) })),
      ),
    );
    return [...links, ...taken].filter((e) => {
      const key = `${e.from}|${e.to}`;
      if (!shapes(relOf(e)) || !visible.has(e.from) || !visible.has(e.to) || seen.has(key) || (e.kind === "label" && others.has(key))) return false;
      seen.add(key);
      return true;
    });
  }, [g.nodes, links, shapes, visible]); // eslint-disable-line react-hooks/exhaustive-deps
  const layoutLinks = layoutEdges;
  const structureKey = useMemo(
    () =>
      JSON.stringify([
        [...visible].sort(),
        layoutEdges.map((e) => [e.from, e.to, e.kind]),
        g.nodes.filter((n) => visible.has(n.id)).map((n) => [n.name.length, badgesOf(n).length, cardOptions.get(n.id)?.length ?? 0]),
        layoutPref.direction,
      ]),
    [visible, layoutEdges, g.nodes, layoutPref.direction],
  );

  useEffect(() => {
    let cancelled = false;
    const nodes = g.nodes.filter((n) => visible.has(n.id));
    elk
      .layout({
        id: "root",
        layoutOptions: {
          "elk.algorithm": "layered",
          // Edges go from child to parent, and parents come first.
          "elk.direction": layoutPref.direction === "down" ? "UP" : "LEFT",
          "elk.layered.spacing.nodeNodeBetweenLayers": "60",
          "elk.spacing.nodeNode": "26",
          "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
          "elk.separateConnectedComponents": "true",
          "elk.spacing.componentComponent": "60",
        },
        children: nodes.map((n) => ({ id: n.id, width: W, height: cardHeight(n) })),
        edges: layoutEdges.map((e, i) => {
          const { parent, child } = ws.info(e.kind)?.reversed ? { parent: e.from, child: e.to } : { parent: e.to, child: e.from };
          return { id: `s${i}`, sources: [child], targets: [parent] };
        }),
      })
      .then((res) => {
        if (cancelled) return;
        const target = new Map((res.children ?? []).map((c) => [c.id, { x: c.x ?? 0, y: c.y ?? 0 }]));
        const ids = nodes.map((n) => n.id);
        setLayout({ ids, target });
        updateNodeInternals(ids);
        cancelAnimationFrame(anim.current);
        const from = posRef.current;

        if (from.size === 0) {
          // First layout: place directly and frame the whole graph. After this the view is
          // only re-fitted on request (F, or the fit button) — never behind your back.
          entering.current = new Set();
          posRef.current = target;
          setPos(target);
          const kids = res.children ?? [];
          const bounds = {
            x: 0,
            y: 0,
            width: Math.max(1, ...kids.map((c) => (c.x ?? 0) + (c.width ?? 0))),
            height: Math.max(1, ...kids.map((c) => (c.y ?? 0) + (c.height ?? 0))),
          };
          requestAnimationFrame(() => fitBounds(bounds, { padding: 0.08, duration: FIT_MS }));
          return;
        }

        // Any change — a graph edit, a filter, focus: glide every card from where it is to
        // where it's going. Cards appearing grow out of a neighbour already on screen; cards
        // disappearing fade out where they stand.
        const start = new Map<string, XY>();
        entering.current = new Set();
        leaving.current = new Map([...from].filter(([id]) => !target.has(id) && ws.byId.has(id)));
        for (const id of ids) {
          const was = from.get(id);
          if (was) start.set(id, was);
          else {
            entering.current.add(id);
            const nb = links.find((e) => (e.from === id && from.has(e.to)) || (e.to === id && from.has(e.from)));
            start.set(id, (nb && from.get(nb.from === id ? nb.to : nb.from)) || target.get(id)!);
          }
        }

        // Keep your place: the selected card (or the one nearest the centre of the view)
        // stays put on screen while everything moves around it.
        const v = getViewport();
        const rect = canvas.current?.getBoundingClientRect();
        let anchor = selectedNode && start.has(selectedNode) && !entering.current.has(selectedNode) ? selectedNode : null;
        if (!anchor && rect) {
          const cx = (rect.width / 2 - v.x) / v.zoom;
          const cy = (rect.height / 2 - v.y) / v.zoom;
          let best = Infinity;
          for (const [id, p] of start) {
            if (entering.current.has(id)) continue;
            const d = (p.x + W / 2 - cx) ** 2 + (p.y - cy) ** 2;
            if (d < best) {
              best = d;
              anchor = id;
            }
          }
        }

        const t0 = performance.now();
        let lastAnchor = anchor ? start.get(anchor)! : null;
        const step = (now: number) => {
          const t = Math.min(1, (now - t0) / ANIM_MS);
          const k = ease(t);
          const next = new Map<string, XY>();
          for (const id of ids) {
            const a = start.get(id)!;
            const b = target.get(id)!;
            next.set(id, { x: a.x + (b.x - a.x) * k, y: a.y + (b.y - a.y) * k });
          }
          posRef.current = next;
          setPos(next);
          if (anchor && lastAnchor) {
            const now_ = next.get(anchor)!;
            const cur = getViewport();
            setViewport({ x: cur.x - (now_.x - lastAnchor.x) * cur.zoom, y: cur.y - (now_.y - lastAnchor.y) * cur.zoom, zoom: cur.zoom });
            lastAnchor = now_;
          }
          if (t < 1) anim.current = requestAnimationFrame(step);
          else {
            entering.current = new Set();
            leaving.current = new Map();
            setPos(new Map(next));
            // Choosing another layout is a new picture, so show all of it. Nothing else re-fits.
            if (refit.current) {
              refit.current = false;
              requestAnimationFrame(() => fitView({ padding: 0.08, duration: FIT_MS }));
            } else ensureVisible(selectedRef.current);
          }
        };
        anim.current = requestAnimationFrame(step);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [structureKey]);

  const rfNodes: RFNode<CardData>[] = useMemo(
    () =>
      [...layout.ids, ...leaving.current.keys()]
        .filter((id) => ws.byId.has(id) && (pos.has(id) || leaving.current.has(id)))
        .map((id) => {
          const node = ws.byId.get(id)!;
          const gone = leaving.current.has(id) && !pos.has(id);
          return {
            id,
            type: "card",
            position: gone ? leaving.current.get(id)! : pos.get(id)!,
            selectable: !gone,
            // Known sizes up front: lets the minimap draw before React Flow measures.
            initialWidth: W,
            initialHeight: cardHeight(node),
            selected: id === selected,
            data: {
              ws,
              node,
              direction: layoutPref.direction,
              selected: id === selected,
              recent: ws.recent.has(id),
              dim: !matches(node),
              entering: entering.current.has(id),
              leaving: gone,
              linking: id === linkDrag?.from || id === linkDrag?.over,
              out: !!reading(node.cells.need)?.never,
              lines: summaryOf(ws, node),
              // A row is called by its key, under the tables it's in.
              title: node.row ? String(node.row.key) : node.name,
              options: cardOptions.get(id) ?? [],
              holds: ws.view!.edges.filter((l) => l.to === id && l.components.includes("part_of") && folded.has(l.from)).map((l) => ws.byId.get(l.from)?.name ?? ""),
              kinds: [
                ...[node, ...(node.parts ?? []).flatMap((p) => ws.byId.get(p) ?? [])].flatMap((n) => (n.row ? [n.row.table] : [])),
                ...ws.view!.edges.filter((l) => l.from === id && l.components.includes("is_a")).map((l) => ws.byId.get(l.to)?.name ?? ""),
                // Something other things are a kind of, or a table something's a row of, says so.
                ...(ws.view!.edges.some((l) => l.to === id && l.components.includes("is_a")) || (!node.row && ws.view!.nodes.some((n) => n.row?.table === node.name)) ? ["kind"] : []),
              ],
            },
          };
        }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layout, pos, ws.byId, ws.recent, selected, q, linkDrag?.from, linkDrag?.over, layoutPref.direction, folded, cardOptions],
  );

  const rfEdges: RFEdge[] = useMemo(
    () =>
      links
        .filter((e) => visible.has(e.from) && visible.has(e.to) && !hiddenRels.has(relOf(e)))
        .map((e) => {
          const key = e.key;
          const isSel = key === selected;
          const hot = isSel || selected === e.from || selected === e.to;
          const arranges = ws.arranging.includes(e.kind);
          // Into what reads it or holds it, as the edge's leading component says.
          const ends = ws.info(e.kind)?.reversed ? { parent: e.from, child: e.to } : { parent: e.to, child: e.from };
          return {
            id: key,
            source: ends.child,
            target: ends.parent,
            // What it is, and what it holds, such as an option's weight. A label says what a formula calls it.
            label: [e.kind === "label" ? String(e.value) : labelOf(e.kind), e.holds].filter(Boolean).join(" · "),
            className: `edge rel-${e.kind} ${arranges ? "" : "free"} ${hot ? "hot" : ""} ${isSel ? "picked" : ""}`,
            animated: e.kind === "depends_on" && hot,
            // Into what reads it or holds it: where it sits on the map doesn't say which way a formula reads.
            markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
            interactionWidth: 16,
            zIndex: hot ? 10 : 0,
          };
        }),
    [links, ws.arranging, ws.info, visible, hiddenRels, selected],
  );

  const shownCount = present.filter(([f]) => !hiddenFacts.has(f)).length;
  const relCount = rels.filter(([r]) => !hiddenRels.has(r)).length;
  const toggle = <T,>(set: Set<T>, v: T) => {
    const next = new Set(set);
    if (next.has(v)) next.delete(v);
    else next.add(v);
    return next;
  };

  return (
    <div className="map">
      <button
        className={`opts-toggle ${opts ? "open" : ""} ${query || hiddenFacts.size || hiddenRels.size || focus ? "narrowed" : ""}`}
        onClick={() => setOpts(!opts)}
        aria-expanded={opts}
        aria-label={opts ? "Hide view options" : "View options"}
        title="View options"
      >
        <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden>
          <path d="M4 7h10M18 7h2M4 17h4M12 17h8" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
          <circle cx="16" cy="7" r="2.2" fill="none" stroke="currentColor" strokeWidth="1.8" />
          <circle cx="10" cy="17" r="2.2" fill="none" stroke="currentColor" strokeWidth="1.8" />
        </svg>
      </button>
      <div className={`map-toolbar nowrap ${opts ? "opts-open" : ""}`}>
        <span className="key-host search-host">
          <Key k="/" />
          <input
            ref={searchRef}
            className={`search ${q ? "active" : ""}`}
            placeholder="Search…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                // Handled here so the app's Esc doesn't also deselect the match you landed on.
                e.stopPropagation();
                setQuery("");
                e.currentTarget.blur();
              } else if (e.key === "Enter" || e.key === "ArrowDown") {
                e.preventDefault();
                stepMatch.current(e.shiftKey ? -1 : 1);
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                stepMatch.current(-1);
              }
            }}
          />
          {q && (
            <span className="search-ctl">
              <span className={`search-count ${found.length ? "" : "none"}`}>
                {found.length ? `${foundAt >= 0 ? `${foundAt + 1}/` : ""}${found.length}` : "none"}
              </span>
              <button className="icon small" title="Previous match (⇧⏎)" disabled={!found.length} onClick={() => stepMatch.current(-1)}>
                ▲
              </button>
              <button className="icon small" title="Next match (⏎)" disabled={!found.length} onClick={() => stepMatch.current(1)}>
                ▼
              </button>
              <button className="icon small" title="Clear search (esc)" onClick={() => setQuery("")}>
                ×
              </button>
            </span>
          )}
        </span>
        <Dropdown label={<>Nodes {shownCount < present.length || showDead ? `${shownCount}/${present.length}` : ""}</>} active={shownCount < present.length}>
          <div className="dd-head">
            Show nodes with
            <span>
              <button className="link" onClick={() => setHiddenFacts(new Set())}>
                all
              </button>
              <button className="link" onClick={() => setHiddenFacts(new Set(present.map(([f]) => f)))}>
                none
              </button>
            </span>
          </div>
          {present.map(([f, n]) => (
            <DdItem key={f} checked={!hiddenFacts.has(f)} onChange={() => setHiddenFacts(toggle(hiddenFacts, f))} color={compColor(f)}>
              <span className="dd-label">{f === "none" ? "· notes" : f === "own" ? "· only their own cells" : `${ws.info(f)?.icon ?? ""} ${labelOf(f)}`}</span>
              <span className="muted">{n}</span>
            </DdItem>
          ))}
          <div className="dd-sep" />
          <DdItem checked={showDead} onChange={() => setShowDead(!showDead)}>
            <span className="dd-label">dropped</span>
          </DdItem>
        </Dropdown>
        <Dropdown label={<>Links {relCount < rels.length ? `${relCount}/${rels.length}` : ""}</>} active={relCount < rels.length}>
          <div className="dd-head">
            Show links
            <span>
              <button className="link" onClick={() => setHiddenRels(new Set())}>
                all
              </button>
              <button className="link" onClick={() => setHiddenRels(new Set(rels.map(([r]) => r).filter((r) => !shapes(r))))}>
                layout only
              </button>
            </span>
          </div>
          <div className="dd-columns muted">
            <span>show</span>
            <span>layout</span>
          </div>
          {kindRels.map(([r, n]) => (
            <DdItem key={r} checked={!hiddenRels.has(r)} onChange={() => setHiddenRels(toggle(hiddenRels, r))} title={ws.info(r)?.about}>
              <span className={`dd-swatch rel-${r}`} />
              <span className="dd-label">{labelOf(r)}</span>
              <span className="muted">{n}</span>
              <LayoutTick on={shapes(r)} name={labelOf(r)} onChange={(on) => setShapes([r], on)} />
            </DdItem>
          ))}
          {labelRels.length > 0 && (
            <DdItem
              checked={labelRels.every(([r]) => !hiddenRels.has(r))}
              onChange={() => {
                const allShown = labelRels.every(([r]) => !hiddenRels.has(r));
                const next = new Set(hiddenRels);
                for (const [r] of labelRels) {
                  if (allShown) next.add(r);
                  else next.delete(r);
                }
                setHiddenRels(next);
              }}
              title={ws.info("label")?.about}
            >
              <span className="dd-swatch rel-label" />
              <span className="dd-label">Label</span>
              <span className="muted">{labelRels.reduce((sum, [, n]) => sum + n, 0)}</span>
              <LayoutTick on={labelRels.every(([r]) => shapes(r))} name="every label" onChange={(on) => setShapes(labelRels.map(([r]) => r), on)} />
            </DdItem>
          )}
          {labelRels.map(([r, n]) => (
            <DdItem key={r} className="dd-sub" checked={!hiddenRels.has(r)} onChange={() => setHiddenRels(toggle(hiddenRels, r))}>
              <span className="dd-label">
                <code>{r.slice(LABEL_REL.length)}</code>
              </span>
              <span className="muted">{n}</span>
              <LayoutTick on={shapes(r)} name={r.slice(LABEL_REL.length)} onChange={(on) => setShapes([r], on)} />
            </DdItem>
          ))}
          <div className="dd-note muted">Hiding links never moves nodes. Only the layout ticks do.</div>
        </Dropdown>
        <label className="toggle" title="Show only the neighbourhood of the selected node">
          focus
          <select value={focus} onChange={(e) => setFocus(Number(e.target.value))}>
            <option value={0}>off</option>
            <option value={1}>1 hop</option>
            <option value={2}>2 hops</option>
            <option value={3}>3 hops</option>
          </select>
        </label>
        <span className="map-hint" title="With a mouse: drag to pan, ⌘-scroll to zoom. Press ? for all keyboard shortcuts">
          scroll to pan · pinch to zoom · hold <kbd>⌘</kbd> for keys
        </span>
      </div>
      {g.nodes.length === 0 ? (
        <div className="empty">
          <h3>An empty workspace</h3>
          <p>
            Add a coarse idea with <b>+ Node</b>, where a name is enough, or connect Claude to <code>{location.origin}/mcp</code> and ask it to
            help you develop one:
          </p>
          <pre>claude mcp add --transport http recollect {location.origin}/mcp</pre>
        </div>
      ) : (
        <div className="map-canvas" ref={canvas}>
          <ReactFlow
            nodes={rfNodes}
            edges={rfEdges}
            nodeTypes={nodeTypes}
            onNodeClick={(_, n) => onSelect(n.id)}
            onNodeDoubleClick={(_, n) => {
              onSelect(n.id);
              centreOn(n.id);
            }}
            // A row read by where it is has no edge of its own to show, so the node whose formula reads it is shown.
            onEdgeClick={(_, e) => onSelect(e.id.startsWith("reads:") ? e.id.slice("reads:".length).split("|")[0] : e.id)}
            onNodeContextMenu={(e, n) => {
              e.preventDefault();
              onContext({ kind: "node", id: n.id, x: e.clientX, y: e.clientY });
            }}
            onEdgeContextMenu={(e, edge) => {
              const parsed = parseEdgeKey(edge.id);
              if (!parsed || !ws.edgeById.has(parsed)) return;
              e.preventDefault();
              onContext({ kind: "edge", edge: parsed, x: e.clientX, y: e.clientY });
            }}
            onPaneClick={() => onSelect(null)}
            nodesConnectable={false}
            nodesDraggable={false}
            // All keys are ours: arrows move the selection and ⌫ asks before dropping.
            disableKeyboardA11y
            deleteKeyCode={null}
            selectionKeyCode={null}
            // On a Mac the trackpad pans, so a click-drag on the canvas does nothing.
            panOnDrag={!IS_MAC}
            // Wheel and pinch are handled by our frame-coalescing listener on the wrapper.
            panOnScroll={false}
            zoomOnScroll={false}
            zoomOnPinch={false}
            zoomOnDoubleClick={false}
            minZoom={0.1}
            maxZoom={2.5}
            proOptions={{ hideAttribution: true }}
          >
            <Background gap={24} size={1} />
            <Controls showInteractive={false} fitViewOptions={{ padding: 0.08, duration: FIT_MS }}>
              <ControlButton
                title={layoutPref.direction === "down" ? "Reading top to bottom. Click for left to right" : "Reading left to right. Click for top to bottom"}
                aria-label={layoutPref.direction === "down" ? "Lay out left to right" : "Lay out top to bottom"}
                onClick={() => setLayoutPref({ ...layoutPref, direction: layoutPref.direction === "down" ? "right" : "down" })}
              >
                {layoutPref.direction === "down" ? "↓" : "→"}
              </ControlButton>
              <ControlButton
                className={layoutPref.fold === false ? "mini-toggle" : "mini-toggle on"}
                title={layoutPref.fold === false ? "Options of several questions are cards. Click to draw them on each question instead" : "Options of several questions are drawn on each question. Click to draw them as cards"}
                aria-pressed={layoutPref.fold !== false}
                onClick={() => setLayoutPref({ ...layoutPref, fold: layoutPref.fold === false })}
              >
                ⋯
              </ControlButton>
              <ControlButton
                className={mini.open ? "mini-toggle on" : "mini-toggle"}
                title={mini.open ? "Hide the minimap (m)" : "Show the minimap (m)"}
                aria-pressed={mini.open}
                onClick={() => setMini({ ...mini, open: !mini.open })}
              >
                ▦
              </ControlButton>
            </Controls>
            {mini.open && <FloatingMiniMap pref={mini} onChange={setMini} canvas={canvas} />}
          </ReactFlow>
          {linkDrag && (
            <svg className="link-preview">
              <defs>
                <marker id="link-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                  <path d="M0 0L10 5L0 10z" />
                </marker>
              </defs>
              <line x1={linkDrag.start.x} y1={linkDrag.start.y} x2={linkDrag.end.x} y2={linkDrag.end.y} markerEnd="url(#link-arrow)" />
            </svg>
          )}
          {linkDrop && (
            <LinkMenu
              key={`${linkDrop.from}-${linkDrop.to}`}
              ws={ws}
              drop={linkDrop}
              onSwap={() => setLinkDrop({ ...linkDrop, from: linkDrop.to, to: linkDrop.from })}
              onDone={(kind) => {
                setLinkDrop(null);
                if (kind) {
                  setHiddenRels((h) => {
                    const next = new Set(h);
                    next.delete(kind);
                    return next;
                  });
                }
              }}
            />
          )}
        </div>
      )}
    </div>
  );
}

export function MapView(props: ViewProps & { active: boolean; onContext: (t: CtxTarget) => void }) {
  return (
    <ReactFlowProvider>
      <MapInner {...props} />
    </ReactFlowProvider>
  );
}
