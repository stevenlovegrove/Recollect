import { useEffect, useLayoutEffect, useRef } from "react";
import type { Workspace } from "./workspace";
import { ViewIcon } from "./ViewNav";
import { WorldPanel, useStepping } from "./views/WorldPanel";

/** Whether there's anything to narrow to: an open choice, or something pinned. */
export function hasWorlds(ws: Workspace) {
  const w = ws.view?.worlds;
  return !!w && (w.vars.length > 0 || (w.quantities ?? []).length > 0);
}

/** What's pinned, each as its choice and option. */
function pinnedList(ws: Workspace): string[] {
  const w = ws.view!.worlds;
  const said = [
    ...w.vars.flatMap((v) => v.options.filter((o) => o.picked).map((o) => `${v.name}: ${o.name}`)),
    ...w.vars.flatMap((v) => v.options.filter((o) => `!${v.id}/${o.id}` in ws.world).map((o) => `${v.name}: not ${o.name}`)),
    ...(w.quantities ?? []).map((q) => `${ws.byId.get(q.node)?.name ?? "something"} ${q.text.includes("..") ? `in ${q.text}` : `= ${q.text}`}`),
  ];
  return said;
}

/** How many worlds, and what's pinned: the chip, and the head of what it grows into. Open, what's
 *  pinned is plain from the choices themselves, so it isn't said again, and pinning one doesn't make
 *  the head grow and move things out from under the pointer. */
function Summary({ ws, open, onToggle }: { ws: Workspace; open: boolean; onToggle: () => void }) {
  const count = ws.view!.worlds.count;
  const said = pinnedList(ws);
  // Short enough for the chip; all of it on hover.
  const pins = open || !said.length ? null : said.length <= 2 ? said.join(", ") : `${said.length} pinned`;
  return (
    <button
      className={`world-chip ${said.length ? "narrowed" : ""}`}
      onClick={onToggle}
      aria-expanded={open}
      title={[open ? "Fold the worlds away (w)" : "Every choice, and what's left (w)", ...said].join("\n")}
    >
      <ViewIcon name="worlds" />
      <span className={`world-chip-count ${count === "0" ? "none" : ""}`}>
        <b>{count === "0" ? "No worlds" : Number(count).toLocaleString()}</b> {count === "1" ? "world" : count === "0" ? "fit" : "worlds"}
      </span>
      {pins && <span className="world-chip-pins">{pins}</span>}
    </button>
  );
}

/**
 * The worlds as a chip in the header: how many there are, and what they're narrowed to. It grows into
 * every choice in place, the same on every page, and folds away again on a click anywhere else. A dot
 * says the page is narrowed while it's folded, so it never looks like all of them.
 */
export function Worlds({ ws, open, onOpen, onSelect }: { ws: Workspace; open: boolean; onOpen: (open: boolean) => void; onSelect: (id: string | null) => void }) {
  const here = useRef<HTMLDivElement>(null);
  const steps = useStepping(ws);
  const card = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!here.current?.contains(e.target as Node)) onOpen(false);
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open, onOpen]);
  // While it's open it only ever grows, wider or taller, so what's under the pointer stays put as choices
  // are pinned, their percentages come and go, and what's worth finding out is answered. It starts again
  // from what it holds each time it opens.
  const biggest = useRef({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = card.current;
    if (!open || !el) {
      biggest.current = { width: 0, height: 0 };
      return;
    }
    el.style.minWidth = "";
    el.style.minHeight = "";
    const box = el.getBoundingClientRect();
    const b = biggest.current;
    b.width = Math.min(Math.max(b.width, box.width), window.innerWidth - 24);
    b.height = Math.min(Math.max(b.height, box.height), window.innerHeight - 70);
    el.style.minWidth = `${b.width}px`;
    el.style.minHeight = `${b.height}px`;
  });
  if (!hasWorlds(ws)) return null;
  const toggle = () => onOpen(!open);
  return (
    <div className={`world-bubble ${open ? "open" : ""}`} ref={here}>
      <Summary ws={ws} open={open} onToggle={toggle} />
      {open && (
        <div className="world-card" ref={card}>
          <WorldPanel ws={ws} onSelect={onSelect} steps={steps} summary={<Summary ws={ws} open={open} onToggle={toggle} />} />
        </div>
      )}
    </div>
  );
}
