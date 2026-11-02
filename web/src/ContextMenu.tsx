import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Op } from "./api";
import type { Workspace } from "./workspace";
import { edgeKey } from "./bits";
import { GLYPH, compColor, isDropped, label } from "./theme";

export type CtxTarget = { x: number; y: number } & ({ kind: "node"; id: string } | { kind: "edge"; from: string; rel: string; to: string });

/** Right-click menu for a node or link: change what it is, or delete it. */
export function ContextMenu({ ws, target, selected, onSelect, onClose }: { ws: Workspace; target: CtxTarget; selected: string | null; onSelect: (id: string | null) => void; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: target.x, top: target.y });
  const [err, setErr] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  // Keep the whole menu on screen.
  useLayoutEffect(() => {
    const r = ref.current?.getBoundingClientRect();
    if (!r) return;
    setPos({
      left: Math.max(8, Math.min(target.x, innerWidth - r.width - 8)),
      top: Math.max(8, Math.min(target.y, innerHeight - r.height - 8)),
    });
  }, [target.x, target.y, confirming, err]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      onClose();
    };
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [onClose]);

  const run = async (ops: Op[], why = "") => {
    try {
      setErr(null);
      await ws.apply(ops, why);
      return true;
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      return false;
    }
  };
  const view = ws.view!;

  let body;
  if (target.kind === "node") {
    const id = target.id;
    const node = ws.byId.get(id);
    if (!node) return null;
    const facts = view.components.filter((c) => c.on === "node" && !c.derived && !["name", "body", "sources", "dropped"].includes(c.name) && (c.initial === null || typeof c.initial === "object"));
    const toggle = (c: string) => run([node.cells[c] ? { op: "set", id, component: c } : { op: "set", id, component: c, value: ws.info(c)?.initial ?? null }]);
    const dropped = isDropped(node);
    body = (
      <>
        <div className="ctx-title" title={node.name}>
          {node.name}
        </div>
        <div className="ctx-label">What it is: it can be several</div>
        {facts.map((c) => (
          <button key={c.name} className="ctx-item" role="menuitemcheckbox" aria-checked={!!node.cells[c.name]} title={c.about} onClick={() => toggle(c.name)}>
            <span className="ctx-check">{node.cells[c.name] ? "✓" : ""}</span>
            <span className="ctx-glyph" style={{ color: compColor(c.name) }}>
              {GLYPH[c.name] ?? "·"}
            </span>
            {label(c.name)}
          </button>
        ))}
        <div className="ctx-sep" />
        {ws.info("dropped") && (
          <button
            className="ctx-item"
            title={dropped ? "Bring it back" : "Take it out of the picture, and keep it"}
            onClick={async () => {
              if (await run([dropped ? { op: "set", id, component: "dropped" } : { op: "set", id, component: "dropped", value: true }])) onClose();
            }}
          >
            <span className="ctx-check" />
            {dropped ? "Bring back" : "Drop"}
          </button>
        )}
        <button
          className={`ctx-item danger ${confirming ? "confirming" : ""}`}
          onClick={async () => {
            if (!confirming) return setConfirming(true);
            if (await run([{ op: "delete_node", id }])) onClose();
          }}
        >
          <span className="ctx-check" />
          {confirming ? "Click again to delete" : "Delete"}
        </button>
      </>
    );
  } else {
    const { from, rel, to } = target;
    const link = view.links.find((l) => l.from === from && l.kind === rel && l.to === to);
    const title = (n: string) => ws.byId.get(n)?.name ?? n;
    // One change-set, so a refused change (say, a deadlock) leaves the original link in place.
    const replace = async (next: { from: string; kind: string; to: string }) => {
      const value = next.kind === rel ? link?.value : ws.info(next.kind)?.initial ?? null;
      const ok = await run([
        { op: "link", from, to, kind: rel },
        { op: "link", ...next, value: value ?? null },
      ]);
      if (ok) {
        if (selected === edgeKey({ from, kind: rel, to })) onSelect(edgeKey(next));
        onClose();
      }
    };
    const taken = (k: string) => view.links.some((l) => l.from === from && l.to === to && l.kind === k);
    body = (
      <>
        <div className="ctx-title" title={`${title(from)} → ${title(to)}`}>
          {title(from)} → {title(to)}
        </div>
        <div className="ctx-label">Kind</div>
        {view.components
          .filter((c) => c.on === "link")
          .map((c) => (
            <button key={c.name} className="ctx-item" role="menuitemradio" aria-checked={c.name === rel} disabled={c.name !== rel && taken(c.name)} title={c.about} onClick={() => (c.name === rel ? onClose() : replace({ from, kind: c.name, to }))}>
              <span className="ctx-check">{c.name === rel ? "✓" : ""}</span>
              <span className={`dd-swatch rel-${c.name}`} />
              {label(c.name)}
            </button>
          ))}
        <div className="ctx-sep" />
        <button className="ctx-item" onClick={() => replace({ from: to, kind: rel, to: from })}>
          <span className="ctx-check" />
          Reverse direction
        </button>
        <button
          className="ctx-item danger"
          onClick={async () => {
            if (await run([{ op: "link", from, to, kind: rel }])) onClose();
          }}
        >
          <span className="ctx-check" />
          Delete
        </button>
      </>
    );
  }

  return (
    <div ref={ref} className="ctx-menu" role="menu" style={pos} onContextMenu={(e) => e.preventDefault()}>
      {body}
      {err && <div className="error">{err}</div>}
    </div>
  );
}
