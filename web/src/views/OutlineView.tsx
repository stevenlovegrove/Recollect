import { useEffect, useMemo, useRef, useState } from "react";
import { isTyping } from "../Shortcuts";
import type { CtxTarget } from "../ContextMenu";
import type { GraphNode } from "../api";
import type { ViewProps } from "./MapView";
import { CompBadges, Truth, cellText, reading } from "../bits";
import { isDropped, isNote, label, nodeColor } from "../theme";

/**
 * Coarse to fine: the links that give the graph its shape, with whatever they point at on top. A
 * node with several parents appears under each, and what's beneath it only expands once.
 */
export function OutlineView({ ws, selected, onSelect, onShowDecisions, onContext }: ViewProps & { onShowDecisions: (id: string) => void; onContext: (t: CtxTarget) => void }) {
  const g = ws.view!;
  const [showDropped, setShowDropped] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const { children, parents, notes } = useMemo(() => {
    const children = new Map<string, { id: string; kind: string }[]>();
    const parents = new Map<string, string[]>();
    const notes = new Map<string, { kind: string; id: string }[]>();
    for (const l of g.links) {
      const shape = ws.shape(l);
      if (shape) {
        const { parent, child } = shape;
        if (!children.get(parent)?.some((c) => c.id === child)) children.set(parent, [...(children.get(parent) ?? []), { id: child, kind: l.kind }]);
        parents.set(child, [...(parents.get(child) ?? []), parent]);
      } else {
        notes.set(l.from, [...(notes.get(l.from) ?? []), { kind: l.kind, id: l.to }]);
      }
    }
    return { children, parents, notes };
  }, [g.links, ws.shape]);

  const visible = (n: GraphNode | undefined): n is GraphNode => !!n && (showDropped || !isDropped(n));
  const liveParents = (id: string) => (parents.get(id) ?? []).filter((p) => visible(ws.byId.get(p)));
  const roots = g.nodes.filter((n) => visible(n) && liveParents(n.id).length === 0 && (children.get(n.id)?.length ?? 0) > 0);
  const loose = g.nodes.filter((n) => visible(n) && liveParents(n.id).length === 0 && !(children.get(n.id)?.length ?? 0));

  // Keyboard: ↑↓ walk the visible rows; ←→ collapse or expand, then move to the parent or first child.
  // A shared node has several rows, so remember which one we're on.
  const cursor = useRef(0);
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.key.startsWith("Arrow") || e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || document.querySelector(".modal, .ctx-menu")) return;
      const rows = [...(body.current?.querySelectorAll<HTMLElement>(".outline-row[data-id]") ?? [])];
      if (!rows.length) return;
      const i = rows[cursor.current]?.dataset.id === selected ? cursor.current : rows.findIndex((r) => r.dataset.id === selected);
      const go = (next: number) => {
        const j = Math.max(0, Math.min(rows.length - 1, next));
        cursor.current = j;
        onSelect(rows[j].dataset.id!);
        rows[j].scrollIntoView({ block: "nearest" });
      };
      e.preventDefault();
      if (i < 0) return go(0);
      const id = rows[i].dataset.id!;
      const hasKids = (children.get(id) ?? []).some((k) => visible(ws.byId.get(k.id)));
      if (e.key === "ArrowUp") go(i - 1);
      else if (e.key === "ArrowDown") go(i + 1);
      else if (e.key === "ArrowRight") {
        if (hasKids && collapsed.has(id)) toggle(id);
        else if (hasKids) go(i + 1);
      } else if (e.key === "ArrowLeft") {
        if (hasKids && !collapsed.has(id)) toggle(id);
        else {
          const parent = liveParents(id)[0];
          const j = parent ? rows.findIndex((r) => r.dataset.id === parent) : -1;
          if (j >= 0) go(j);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const expanded = new Set<string>();
  const toggle = (id: string) => {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setCollapsed(next);
  };
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;

  const row = (id: string, depth: number, parent?: string): React.ReactNode => {
    const n = ws.byId.get(id);
    if (!visible(n)) return null;
    const kids = (children.get(id) ?? []).filter((k) => visible(ws.byId.get(k.id)));
    const first = !expanded.has(id);
    expanded.add(id);
    const others = liveParents(id).filter((p) => p !== parent);
    const isCollapsed = collapsed.has(id) || !first;
    const need = reading(n.cells.need);
    const finished = n.cells.done?.shown?.kind === "cond" && n.cells.done.shown.cases.every((c) => c.value === true);
    return (
      <div key={`${parent ?? "root"}/${id}`}>
        <div data-id={id} className={`outline-row ${id === selected ? "selected" : ""} ${ws.recent.has(id) ? "recent" : ""} ${isDropped(n) ? "dead" : ""} ${finished ? "resolved" : ""} ${need?.never ? "out" : ""}`} style={{ paddingLeft: 12 + depth * 22 }}>
          <button className="twisty" disabled={!kids.length || !first} onClick={() => toggle(id)}>
            {kids.length ? (isCollapsed ? "▸" : "▾") : "·"}
          </button>
          <span className="lane" style={{ background: nodeColor(n) }} />
          <button className="outline-title" onClick={() => onSelect(id)}>
            {n.name}
          </button>
          <CompBadges n={n} />
          {g.components
            // What it could start on says less than whether it's needed, and there's only one line.
            .filter((c) => c.summary && c.name !== "actionable" && n.cells[c.name])
            .map((c) => {
              const r = reading(n.cells[c.name]);
              if (r?.always) return null;
              return (
                <span key={c.name} className="outline-value">
                  <span className="outline-value-k">{label(c.name)}</span> {r ? <Truth component={c.name} r={r} /> : cellText(n.cells[c.name], nameOf)}
                </span>
              );
            })}
          {others.length > 0 && (
            <span className="shared" title={others.map((o) => nameOf(o)).join("\n")}>
              ⑂ also under {others.length}
            </span>
          )}
          {!first && kids.length > 0 && <span className="muted small">(expanded above)</span>}
          {(notes.get(id) ?? []).map((a) => {
            const o = ws.byId.get(a.id);
            if (!o) return null;
            return (
              <button key={a.kind + a.id} className="annotation" style={{ borderColor: nodeColor(o) }} title={`${label(a.kind)}: ${o.name}`} onClick={() => onSelect(a.id)}>
                {label(a.kind)} {o.name.slice(0, 28)}
                {o.name.length > 28 ? "…" : ""}
              </button>
            );
          })}
          {n.cells.decision && (
            <button className="link small" onClick={() => onShowDecisions(id)}>
              options →
            </button>
          )}
        </div>
        {!isCollapsed && kids.map((k) => row(k.id, depth + 1, id))}
      </div>
    );
  };

  return (
    <div className="outline">
      <div className="map-toolbar">
        <span className="muted">Coarse to fine: what's on top is what the rest adds up to.</span>
        {ws.info("dropped") && (
          <label className="toggle">
            <input type="checkbox" checked={showDropped} onChange={(e) => setShowDropped(e.target.checked)} /> dropped
          </label>
        )}
      </div>
      <div
        className="outline-body"
        ref={body}
        onContextMenu={(e) => {
          const id = (e.target as Element).closest<HTMLElement>(".outline-row[data-id]")?.dataset.id;
          if (!id) return;
          e.preventDefault();
          onContext({ kind: "node", id, x: e.clientX, y: e.clientY });
        }}
      >
        {roots.length === 0 && <p className="muted pad">Nothing has any structure yet. Add a part to a node to start one.</p>}
        {roots.map((r) => row(r.id, 0))}
        {loose.length > 0 && (
          <>
            <h4 className="outline-section">On their own ({loose.length})</h4>
            {loose.map((n) => (
              <div key={n.id} data-id={n.id} className={`outline-row ${n.id === selected ? "selected" : ""}`} style={{ paddingLeft: 34 }}>
                <span className="lane" style={{ background: nodeColor(n) }} />
                <button className="outline-title" onClick={() => onSelect(n.id)}>
                  {n.name}
                </button>
                <CompBadges n={n} />
                {isNote(n) && <span className="muted small">note</span>}
                {(notes.get(n.id) ?? []).map((a) => (
                  <button key={a.kind + a.id} className="annotation" onClick={() => onSelect(a.id)}>
                    {label(a.kind)} {ws.byId.get(a.id)?.name.slice(0, 28)}
                  </button>
                ))}
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}
