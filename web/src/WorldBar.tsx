import type { Workspace } from "./workspace";
import { ViewIcon } from "./ViewNav";
import { WorldPanel } from "./views/WorldPanel";

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
    ...(w.quantities ?? []).map((q) => `${ws.byId.get(q.node)?.name ?? "something"} ${q.text.includes("..") ? `in ${q.text}` : `= ${q.text}`}`),
  ];
  return said;
}

/**
 * The worlds as a chip: how many there are, and what they're narrowed to. It opens into the panel of
 * every choice, the same on every page. A dot says the page is narrowed even while it's folded, so it
 * never looks like all of them.
 */
export function WorldChip({ ws, open, onToggle }: { ws: Workspace; open: boolean; onToggle: () => void }) {
  const count = ws.view!.worlds.count;
  const said = pinnedList(ws);
  // Short enough for the chip; all of it on hover.
  const pins = !said.length ? null : said.length <= 2 ? said.join(", ") : `${said.length} pinned`;
  return (
    <button
      className={`world-chip ${open ? "open" : ""} ${pins ? "narrowed" : ""}`}
      onClick={onToggle}
      aria-expanded={open}
      title={[open ? "Hide the worlds (w)" : "Every choice, and what's left (w)", ...said].join("\n")}
    >
      <ViewIcon name="worlds" />
      <span className="world-chip-count">
        <b>{count === "0" ? "No worlds" : Number(count).toLocaleString()}</b> {count === "1" ? "world" : count === "0" ? "fit" : "worlds"}
      </span>
      {pins && <span className="world-chip-pins">{pins}</span>}
    </button>
  );
}

/** The chip and, when it's open, the panel under it. On the map it floats; elsewhere it heads the page. */
export function Worlds({ ws, open, onOpen, onSelect, floating }: { ws: Workspace; open: boolean; onOpen: (open: boolean) => void; onSelect: (id: string | null) => void; floating?: boolean }) {
  if (!hasWorlds(ws)) return null;
  const chip = <WorldChip ws={ws} open={open} onToggle={() => onOpen(!open)} />;
  const panel = open && (
    <div className="world-pop">
      <WorldPanel ws={ws} onSelect={onSelect} onClose={() => onOpen(false)} />
    </div>
  );
  if (floating) {
    return (
      <>
        {chip}
        {panel}
      </>
    );
  }
  return (
    <div className="world-strip">
      {chip}
      {panel}
    </div>
  );
}
