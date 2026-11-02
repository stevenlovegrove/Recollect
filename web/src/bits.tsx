import { useMemo, useState } from "react";
import type { Cell, GraphNode, Link, Op, Reading, Shown } from "./api";
import { bracketRuns, pairText } from "./brackets";
import type { Workspace } from "./workspace";
import { GLYPH, compColor, factsOf, isDropped, label, nodeColor } from "./theme";

export function NodeChip({ ws, id, onSelect, fallback }: { ws: Workspace; id: string; onSelect: (id: string) => void; fallback?: string }) {
  const n = ws.byId.get(id);
  if (!n && fallback) return <span className="chip new">{fallback}</span>;
  return (
    <button className={`chip ${n ? (isDropped(n) ? "dead" : "") : "missing"}`} onClick={() => n && onSelect(id)} title={n ? factsOf(n).join(", ") || "note" : "not in this graph"}>
      <span className="chip-dot" style={{ background: n ? nodeColor(n) : "var(--danger)" }} />
      {n?.name ?? `${id.slice(-6)}`}
    </button>
  );
}

export function CompBadges({ n }: { n: GraphNode }) {
  return (
    <span className="badges">
      {factsOf(n).map((c) => (
        <CompBadge key={c} c={c} />
      ))}
    </span>
  );
}

export function CompBadge({ c }: { c: string }) {
  return (
    <span className="cbadge" style={{ ["--c" as string]: compColor(c) }} title={c}>
      {GLYPH[c] && <span className="glyph">{GLYPH[c]}</span>}
      {label(c)}
    </span>
  );
}

/** A plain value, in as few words as it can say it. */
export function valueText(v: unknown): string {
  if (v === null || v === undefined) return "yes";
  if (typeof v === "number") return Number.isInteger(v) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  if (typeof v === "string") return v;
  if (typeof v === "boolean") return v ? "yes" : "no";
  return JSON.stringify(v);
}

/** Numbers as a range, and anything else as a short list. Worlds are the point, so never just one of them. */
function casesText(values: unknown[], names?: (s: string) => string): string {
  if (values.length === 1) return displayValue(values[0], names);
  // In words, because a dash between numbers reads as subtracting them.
  const ns = values.filter((v) => typeof v === "number") as number[];
  if (ns.length > 1 && ns.length >= values.length - 1 && values.every((v) => typeof v === "number" || v === null)) {
    const range = `${valueText(Math.min(...ns))} to ${valueText(Math.max(...ns))}`;
    return ns.length === values.length ? range : `${range}, or undefined`;
  }
  const shown = values.map((v) => displayValue(v, names));
  return shown.length > 3 ? `${shown.slice(0, 3).join(" / ")} / …` : shown.join(" / ");
}

function displayValue(v: unknown, names?: (s: string) => string): string {
  if (v === null) return "undefined";
  if (typeof v === "string" && names) return names(v);
  return valueText(v);
}

/** A cell in one line, for a card or a table. */
export function cellText(cell: Cell, names?: (s: string) => string): string {
  if (cell.error) return "error";
  if (cell.shown) return shownText(cell.shown, names);
  return valueText(cell.value);
}

export function shownText(s: Shown, names?: (s: string) => string): string {
  switch (s.kind) {
    case "bool":
      return s.when;
    case "cond":
      return casesText(s.cases.map((c) => c.value), names);
    case "named":
      return s.items.length ? s.items.map((i) => names?.(i.node) ?? i.node).join(", ") : "nothing";
  }
}

/** Nearest the pointer first, then on out to the outermost. */
const HELD_COLORS = ["var(--accent)", "var(--c-goal)", "var(--c-task)", "var(--c-question)"];

/** Text with its brackets lit by the pointer: the pair holding it, and every pair around that. */
export function Brackets({ text, onHeld, renderText }: { text: string; /** The innermost pair the pointer is in, if there is one. */ onHeld?: (part: string | null) => void; /** How to show what's between the brackets. */ renderText?: (text: string) => React.ReactNode }) {
  const runs = useMemo(() => bracketRuns(text), [text]);
  const [over, setOver] = useState<number | null>(null);
  if (!runs.some((r) => r.bracket)) return <>{renderText ? renderText(text) : text}</>;
  const held = (over !== null && runs[over]?.held) || [];
  const point = (i: number | null) => {
    setOver(i);
    const open = i === null ? undefined : runs[i]?.held[runs[i].held.length - 1];
    onHeld?.(open === undefined ? null : pairText(text, runs, open));
  };
  return (
    <span onMouseLeave={() => point(null)}>
      {runs.map((r, i) => {
        const nth = r.bracket ? held.length - 1 - held.indexOf(r.held[r.held.length - 1]) : held.length;
        const lit = nth < held.length;
        return (
          <span key={i} className={lit ? "bracket-held" : undefined} style={lit ? { color: HELD_COLORS[nth % HELD_COLORS.length] } : undefined} onMouseEnter={() => point(i)}>
            {renderText && !r.bracket ? renderText(r.text) : r.text}
          </span>
        );
      })}
    </span>
  );
}

/** Past this, a formula shows its first alternatives and how many more there are. */
const LONG_FORMULA = 240;

export function Formula({ r }: { r: Reading | string }) {
  const text = typeof r === "string" ? r : r.when;
  const [whole, setWhole] = useState(false);
  const alternatives = text.split(" ∨ ");
  let kept = 1;
  while (kept < alternatives.length && alternatives.slice(0, kept + 1).join(" ∨ ").length <= LONG_FORMULA) kept++;
  const cut = !whole && kept < alternatives.length;
  return (
    <span className={`formula ${text === "always" ? "always" : text === "never" ? "never" : ""}`}>
      <Brackets text={cut ? alternatives.slice(0, kept).join(" ∨ ") : text} />
      {cut && (
        <>
          {" "}
          <button
            className="link small"
            onClick={(e) => {
              e.stopPropagation();
              setWhole(true);
            }}
          >
            ∨ {(alternatives.length - kept).toLocaleString()} more
          </button>
        </>
      )}
    </span>
  );
}

/**
 * How a true-or-false cell reads, the same everywhere. Something like "Needed" is already a question,
 * so it only needs when. A value has to say what it is.
 */
export function truthWords(component: string, r: Reading): { fixed: string } | { lead: string } {
  const isValue = component === "value";
  if (r.always) return { fixed: isValue ? "true" : "always" };
  if (r.never) return { fixed: isValue ? "false" : "never" };
  return { lead: isValue ? "true when" : "when" };
}

export function Truth({ component, r }: { component: string; r: Reading }) {
  const words = truthWords(component, r);
  if ("fixed" in words) return <span className={`formula ${r.always ? "always" : "never"}`}>{words.fixed}</span>;
  return (
    <>
      <span className="muted">{words.lead}</span> <Formula r={r} />
    </>
  );
}

/** What a value means, for the few that aren't plain. */
const MEANING: Record<string, (v: unknown) => string | undefined> = {
  done: (v) => (v === true ? "finished" : v === false ? "still owed" : "nothing to finish"),
};

/** A cell in full: every case it has among the worlds in play. */
export function CellValue({ ws, cell, onSelect, component }: { ws: Workspace; cell: Cell; onSelect: (id: string) => void; component?: string }) {
  const meaning = component ? MEANING[component] : undefined;
  const name = (s: string) => ws.byId.get(s)?.name ?? s;
  const shownValue = (v: unknown) => meaning?.(v) ?? displayValue(v, name);
  if (cell.error) return <span className="cell-error">{cell.error}</span>;
  const s = cell.shown;
  if (!s) {
    if (typeof cell.value === "string" && ws.byId.has(cell.value)) return <NodeChip ws={ws} id={cell.value} onSelect={onSelect} />;
    return <span className="mono-value">{valueText(cell.value)}</span>;
  }
  if (s.kind === "bool") return component ? <Truth component={component} r={s} /> : <Formula r={s} />;
  if (s.kind === "named") {
    if (!s.items.length) return <span className="muted">nothing</span>;
    return (
      <span className="named">
        {s.items.map((i) => (
          <span key={i.node} className="named-item">
            <NodeChip ws={ws} id={i.node} onSelect={onSelect} /> <Formula r={i.when} />
          </span>
        ))}
      </span>
    );
  }
  if (s.cases.length === 1) return <span className="mono-value">{shownValue(s.cases[0].value)}</span>;
  return (
    <span className="cases">
      {s.cases.map((c, i) => (
        <span key={i} className="case">
          <span className="mono-value">{shownValue(c.value)}</span> <span className="muted">when</span> <Formula r={c.when} />
        </span>
      ))}
    </span>
  );
}

/** What a formula comes to, written as following from it. */
export function Resolved({ ws, cell, component }: { ws: Workspace; cell: Cell; component?: string }) {
  const name = (s: string) => ws.byId.get(s)?.name ?? s;
  const meaning = component ? MEANING[component] : undefined;
  const read = (v: unknown) => (v === null ? "undefined" : (meaning?.(v) ?? displayValue(v, name)));
  const row = (key: number, value: string, when?: Reading) => (
    <div key={key} className="resolved-row">
      <span className="resolved-eq">→</span> <span className="mono-value">{value}</span>
      {when && (
        <>
          {" "}
          <span className="muted">when</span> <Formula r={when} />
        </>
      )}
    </div>
  );
  if (cell.error) return <div className="cell-error resolved">{cell.error}</div>;
  const s = cell.shown;
  if (!s) return <div className="resolved">{row(0, read(cell.value))}</div>;
  if (s.kind === "bool") return <div className="resolved">{s.always ? row(0, "true") : s.never ? row(0, "false") : row(0, "true", s)}</div>;
  if (s.kind === "named") return <div className="resolved">{row(0, shownText(s, name))}</div>;
  if (s.cases.length === 1) return <div className="resolved">{row(0, read(s.cases[0].value))}</div>;
  return <div className="resolved">{s.cases.map((c, i) => row(i, read(c.value), c.when))}</div>;
}

/** A world value as a bool, if that's what it is. */
export function reading(cell: Cell | undefined): Reading | undefined {
  return cell?.shown?.kind === "bool" ? cell.shown : undefined;
}

/** The values a cell can still take. */
export function casesOf(cell: Cell | undefined): unknown[] {
  if (!cell) return [];
  if (cell.shown?.kind === "cond") return cell.shown.cases.map((c) => c.value);
  return cell.value === undefined ? [] : [cell.value];
}

/** The change a tick makes: what the node says for itself goes first, and otherwise it says the opposite of what's worked out. */
export function tickOp(id: string, c: string, cell: Cell): Op {
  if (cell.own) return { op: "set", id, component: c };
  const cases = casesOf(cell);
  return { op: "set", id, component: c, value: !(cases.length > 0 && cases.every((v) => v === true)) };
}

/** A true-or-false cell as a box to tick. A node's own formula decides for itself, and differing worlds show as neither. */
export function TickBox({ ws, id, c }: { ws: Workspace; id: string; c: string }) {
  const cell = ws.byId.get(id)?.cells[c];
  if (!cell) return null;
  const cases = casesOf(cell);
  const yes = cases.length > 0 && cases.every((v) => v === true);
  const mixed = !yes && cases.includes(true);
  const decided = cell.own && cell.formula !== undefined;
  return (
    <input
      type="checkbox"
      aria-label={label(c)}
      checked={yes}
      disabled={decided}
      ref={(el) => {
        if (el) el.indeterminate = mixed;
      }}
      title={decided ? "Its own formula decides this" : cell.own ? "Go back to working this out" : yes ? `Say it isn't ${label(c).toLowerCase()}` : `Mark as ${label(c).toLowerCase()}`}
      onClick={(e) => e.stopPropagation()}
      onChange={() => ws.apply([tickOp(id, c, cell)]).catch((e) => alert(String(e)))}
    />
  );
}

/** Selection keys: nodes are handles, so a link can be keyed as "e:from|kind|to". */
export const edgeKey = (e: Pick<Link, "from" | "kind" | "to">) => `e:${e.from}|${e.kind}|${e.to}`;

export function parseEdgeKey(key: string | null): { from: string; kind: string; to: string } | null {
  if (!key?.startsWith("e:")) return null;
  const [from, kind, to] = key.slice(2).split("|");
  return from && kind && to ? { from, kind, to } : null;
}
