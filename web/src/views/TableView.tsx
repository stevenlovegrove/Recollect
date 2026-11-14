import { useEffect, useMemo, useState } from "react";
import type { Cell, GraphNode } from "../api";
import type { ViewProps } from "./MapView";
import { Truth, cellText, reading, truthWords } from "../bits";
import { entryText, parseEntry } from "../cellEntry";
import { entryOp, holdsFormula } from "../Inspector";
import { DdItem, Dropdown } from "../Dropdown";
import { label, nodeColor } from "../theme";
import { TableGrid, tablesIn } from "./TableGrid";

/**
 * Rows are nodes and columns are cells, as they really are. Typed-in numbers edit in place. A table a
 * workspace's names make can be shown on its own, as a grid of its rows.
 */
export function TableView(props: ViewProps & { /** A table asked for from elsewhere; `n` changes each time one is. */ focus?: { name: string; n: number } | null }) {
  const g = props.ws.view!;
  const tables = useMemo(() => tablesIn(g.nodes), [g.nodes]);
  const [showing, setShowing] = useState<string>(props.focus?.name ?? "");
  useEffect(() => {
    if (props.focus) setShowing(props.focus.name);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.focus?.n]);
  const table = tables.includes(showing) ? showing : "";
  const pick = tables.length > 0 && (
    <select className="small" value={table} onChange={(e) => setShowing(e.target.value)} aria-label="Which table">
      <option value="">Every node</option>
      {tables.map((t) => (
        <option key={t} value={t}>
          {t}
        </option>
      ))}
    </select>
  );
  if (table) {
    return (
      <div className="scroll pad">
        <div className="map-toolbar flat">
          {pick}
          <span className="muted">Faded cells are taken from the table, or from another part of the same thing. Typing in one says it for that row alone.</span>
        </div>
        <TableGrid {...props} table={table} />
      </div>
    );
  }
  return <EveryNode {...props} pick={pick} />;
}

function EveryNode({ ws, selected, onSelect, pick }: ViewProps & { pick: React.ReactNode }) {
  const g = ws.view!;
  const all = useMemo(() => {
    const present = new Set(g.nodes.flatMap((n) => Object.keys(n.cells)));
    present.delete("name");
    present.delete("body");
    // The systems' cells first, in the order they say them, and then the workspace's own.
    const known = g.cells.filter((c) => !c.quiet && present.has(c.name)).map((c) => c.name);
    const own = [...present].filter((c) => !ws.cellInfo(c)).sort();
    return [...known, ...own];
  }, [g.nodes, g.cells, ws]);
  // What a card would show, and what's typed in. Everything else is a click away.
  const preferred = useMemo(() => {
    const summaries = all.filter((c) => !ws.cellInfo(c) || ws.cellInfo(c)!.summary);
    return summaries.length ? summaries : all.slice(0, 4);
  }, [all, ws]);
  const [hidden, setHidden] = useState<Set<string> | null>(null);
  const columns = all.filter((c) => (hidden ? !hidden.has(c) : preferred.includes(c)));
  const toggle = (c: string) => {
    const base = hidden ?? new Set(all.filter((x) => !preferred.includes(x)));
    const next = new Set(base);
    if (next.has(c)) next.delete(c);
    else next.add(c);
    setHidden(next);
  };

  return (
    <div className="scroll pad">
      <div className="map-toolbar flat">
        {pick}
        <Dropdown label={<>Columns {columns.length}/{all.length}</>}>
          {all.map((c) => (
            <DdItem key={c} checked={columns.includes(c)} onChange={() => toggle(c)} title={ws.cellInfo(c)?.about}>
              <span className="dd-label">{label(c)}</span>
            </DdItem>
          ))}
        </Dropdown>
        <span className="muted">Amber cells just ran. Everything else came from memory.</span>
      </div>
      <table className="sheet">
        <thead>
          <tr>
            <th>Node</th>
            {columns.map((c) => (
              <th key={c} title={ws.cellInfo(c)?.about}>
                {label(c)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {g.nodes.map((n) => (
            <tr key={n.id} className={`${n.id === selected ? "selected" : ""} ${reading(n.cells.need)?.never ? "pruned" : ""}`} onClick={() => onSelect(n.id)}>
              <td className="sheet-name">
                <span className="chip-dot" style={{ background: nodeColor(n) }} /> {n.name}
              </td>
              {columns.map((c) => (
                <td key={c} className={`sheet-cell ${n.cells[c]?.error ? "err" : ""} ${ws.recentCells.has(`${n.id}|${c}`) ? "ran" : ""} ${n.cells[c]?.formula !== undefined ? "formula-cell" : ""}`}>
                  <TableCell ws={ws} node={n} c={c} cell={n.cells[c]} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** `written` shows a formula as it's written rather than what it comes to, as a table's own row does. */
export function TableCell({ ws, node, c, cell, written, readOnly }: { ws: ViewProps["ws"]; node: GraphNode; c: string; cell: Cell | undefined; written?: boolean; readOnly?: boolean }) {
  if (!cell) return null;
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;
  const r = reading(cell);
  // What can hold a formula is typed into, as in a spreadsheet, unless it's worked out and can't be.
  if (!readOnly && holdsFormula(ws, c) && (cell.source === "own" || cell.formula !== undefined)) {
    const words = r && truthWords(c, r);
    const shown = written && cell.formula !== undefined ? `=${cell.formula}` : cell.error ? "#ERROR" : !words ? cellText(cell, nameOf) : "fixed" in words ? words.fixed : `${words.lead} ${r!.when}`;
    return <EntryCell key={`${cell.formula}|${JSON.stringify(cell.value)}|${shown}`} ws={ws} node={node} c={c} cell={cell} shown={shown} />;
  }
  if (cell.error) return <span title={cell.error}>#ERROR</span>;
  if (r) return <Truth component={c} r={r} />;
  // A list of words, such as a choice's options, reads as a list.
  if (Array.isArray(cell.value) && cell.value.every((v) => typeof v === "string")) return <span>{cell.value.join(", ")}</span>;
  return <span>{cellText(cell, nameOf)}</span>;
}

/** Shows what a cell comes to, and what it holds while it's being edited: a value, or `=` and a formula. */
function EntryCell({ ws, node, c, cell, shown }: { ws: ViewProps["ws"]; node: GraphNode; c: string; cell: Cell; shown: string }) {
  const [text, setText] = useState<string | null>(null);
  const [refused, setRefused] = useState<string | null>(cell.error ?? null);
  const commit = async () => {
    if (text === null) return;
    if (text.trim() === entryText(cell)) return setText(null);
    try {
      // Names it already has keep their meaning. New ones are said in the inspector, which refusing here points to.
      await ws.apply([entryOp({ id: node.id }, c, parseEntry(text), undefined, cell.formula !== undefined)]);
      setRefused(null);
      setText(null);
    } catch (e) {
      setRefused(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <>
      <input
        className={`sheet-input ${refused ? "bad" : ""}`}
        aria-label={`${node.name}: ${label(c)}`}
        title={cell.formula !== undefined ? `=${cell.formula}` : "A value, or = and a formula"}
        value={text ?? shown}
        onClick={(e) => e.stopPropagation()}
        onFocus={() => setText(entryText(cell))}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") {
            e.stopPropagation();
            setRefused(null);
            setText(null);
          }
        }}
      />
      {refused && (
        <div className="cell-error small">
          {refused}
          {refused.includes("no input called") && ". Say what a new name refers to in the inspector."}
        </div>
      )}
    </>
  );
}
