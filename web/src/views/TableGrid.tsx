import { useEffect, useMemo, useState } from "react";
import { api, type GraphNode, type View } from "../api";
import type { ViewProps } from "./MapView";
import { TableCell } from "./TableView";
import { LinePlot, numberOf } from "./SweepChart";
import { label } from "../theme";

/** Numbered rows in order, then the rest as they were made. */
function byRow(a: GraphNode, b: GraphNode): number {
  const [x, y] = [a.row!.key, b.row!.key];
  if (typeof x === "number" && typeof y === "number") return x - y;
  return Number(typeof x !== "number") - Number(typeof y !== "number");
}

/** The tables in a workspace: every name that a node's name makes a row of. */
export function tablesIn(nodes: GraphNode[]): string[] {
  return [...new Set(nodes.flatMap((n) => (n.row ? [n.row.table] : [])))].sort();
}

/**
 * One table as a grid, the way it's written: its rows are the nodes named `Table[key]`, its columns are the
 * cells it and the kinds above it say, and the top row is the table's own, which every row takes unless it
 * says otherwise.
 */
export function TableGrid({ ws, table, selected, onSelect }: ViewProps & { table: string }) {
  const g = ws.view!;
  const head = g.nodes.find((n) => n.name === table && !n.row);
  const rows = useMemo(() => g.nodes.filter((n) => n.row?.table === table).sort(byRow), [g.nodes, table]);
  // The table and every kind above it: what's said there is this table's to show.
  const lineage = useMemo(() => {
    const seen = new Set<string>();
    const todo = head ? [head.id] : [];
    while (todo.length) {
      const k = todo.pop()!;
      if (seen.has(k)) continue;
      seen.add(k);
      todo.push(...g.links.filter((l) => l.from === k && l.kind === "is_a").map((l) => l.to));
    }
    return seen;
  }, [g.links, head]);
  // A row's other parts say what's theirs to say, in their own tables.
  const columns = useMemo(() => {
    const cs = new Set(Object.entries(head?.cells ?? {}).filter(([, c]) => c.workspace).map(([k]) => k));
    for (const r of rows) for (const [k, c] of Object.entries(r.cells)) if (c.workspace && (c.own || (c.kind && lineage.has(c.kind)))) cs.add(k);
    return [...cs].sort();
  }, [head, rows, lineage]);
  const numbered = rows.length > 0 && rows.every((r) => typeof r.row!.key === "number");

  return (
    <>
      <table className="sheet table-grid">
        <thead>
          <tr>
            <th>{table}</th>
            {columns.map((c) => (
              <th key={c}>{label(c)}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {head && (
            <tr className={`table-head-row ${head.id === selected ? "selected" : ""}`} onClick={() => onSelect(head.id)}>
              <td className="sheet-name" title="What every row takes, unless it says otherwise">
                Every row
              </td>
              {columns.map((c) => (
                <td key={c} className={`sheet-cell ${head.cells[c]?.source === "rule" ? "formula-cell" : ""}`}>
                  <TableCell ws={ws} node={head} c={c} cell={head.cells[c]} written />
                </td>
              ))}
            </tr>
          )}
          {rows.map((r) => (
            <tr key={r.id} className={r.id === selected ? "selected" : ""} onClick={() => onSelect(r.id)}>
              <td className="sheet-name" title={r.name}>
                {String(r.row!.key)}
              </td>
              {columns.map((c) => {
                const cell = r.cells[c];
                // Taken from the table, or from another part of the same thing, rather than said here.
                const taken = cell && !cell.own;
                return (
                  <td key={c} className={`sheet-cell ${taken ? "taken" : ""} ${cell?.error ? "err" : ""} ${ws.recentCells.has(`${r.id}|${c}`) ? "ran" : ""}`} title={taken && cell.kind ? `From ${ws.byId.get(cell.kind)?.name ?? "elsewhere"}` : undefined}>
                    <TableCell ws={ws} node={r} c={c} cell={cell} />
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      <NewRow ws={ws} table={table} rows={rows} />
      {numbered && <RowChart ws={ws} rows={rows} columns={columns} />}
    </>
  );
}

/** Adds a row. One more number for a numbered table. If every row is an option of one decision, so is this. */
function NewRow({ ws, table, rows }: { ws: ViewProps["ws"]; table: string; rows: GraphNode[] }) {
  const keys = rows.map((r) => r.row!.key);
  const next = keys.length && keys.every((k) => typeof k === "number") ? Math.max(...(keys as number[])) + 1 : "";
  const [key, setKey] = useState<string | null>(null);
  const [refused, setRefused] = useState<string | null>(null);
  const g = ws.view!;
  const decisions = rows.map((r) => g.links.filter((l) => l.from === r.id && l.kind === "alternative").map((l) => l.to));
  const shared = decisions.length && decisions[0].length === 1 && decisions.every((d) => d.length === 1 && d[0] === decisions[0][0]) ? decisions[0][0] : null;
  const add = async () => {
    const k = (key ?? String(next)).trim();
    if (!k) return;
    try {
      await ws.apply([{ op: "add_node", id: "new:row", name: `${table}[${k}]`, ...(shared ? { links: [{ to: shared, kind: "alternative", value: {} }] } : {}) }]);
      setKey(null);
      setRefused(null);
    } catch (e) {
      setRefused(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="table-new small">
      <input aria-label={`A new row of ${table}`} placeholder="key" value={key ?? String(next)} onChange={(e) => setKey(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
      <button onClick={add}>Add row</button>
      {shared && <span className="muted">It'll be an option of {ws.byId.get(shared)?.name}, like the rest.</span>}
      {refused && <span className="cell-error">{refused}</span>}
    </div>
  );
}

/**
 * A column down the rows. If it waits on a choice, there's a line for each option, each worked out with that
 * option pinned, so a choice part way down shows where the lines part.
 */
function RowChart({ ws, rows, columns }: { ws: ViewProps["ws"]; rows: GraphNode[]; columns: string[] }) {
  const g = ws.view!;
  const vars = g.worlds.vars;
  const numeric = columns.filter((c) => rows.some((r) => numberOf(r.cells[c], vars) !== null || r.cells[c]?.shown?.kind === "cond"));
  const [chosen, setChosen] = useState<string | null>(null);
  const column = numeric.includes(chosen ?? "") ? chosen! : numeric.find((c) => rows.some((r) => r.cells[c]?.formula)) ?? numeric[0];
  // The first choice still to be made that the column waits on.
  const choice = vars.find((v) => !v.uncertain && rows.some((r) => r.cells[column ?? ""]?.vars?.includes(v.id)));
  const [pinned, setPinned] = useState<Record<string, View> | null>(null);
  const key = `${g.last_seq}|${JSON.stringify(ws.world)}|${choice?.id}`;
  useEffect(() => {
    if (!choice) return setPinned(null);
    let alive = true;
    Promise.all(choice.options.filter((o) => o.possible).map((o) => api.view({ ...ws.world, [choice.id]: o.id }).then((v) => [o.id, v] as const)))
      .then((rs) => alive && setPinned(Object.fromEntries(rs)))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!column) return null;

  const xs = rows.map((r) => r.row!.key as number);
  const down = (v: View) => rows.map((r) => numberOf(v.nodes.find((n) => n.id === r.id)?.cells[column], v.worlds.vars));
  const options = choice ? choice.options.filter((o) => pinned?.[o.id]) : [];
  const [names, ys] = choice ? [options.map((o) => o.name), pinned ? options.map((o) => down(pinned[o.id])) : null] : [[label(column)], [down(g)]];

  return (
    <div className="sweep table-chart">
      <div className="sweep-controls small">
        <select value={column} onChange={(e) => setChosen(e.target.value)} aria-label="Which column to chart">
          {numeric.map((c) => (
            <option key={c} value={c}>
              {label(c)}
            </option>
          ))}
        </select>
        <span className="muted">down the rows{choice ? `, for each answer to ${choice.name}` : ""}</span>
      </div>
      <LinePlot xs={xs} ys={ys} names={names} xName="Row" label={`${label(column)} down the rows${choice ? ` for each answer to ${choice.name}` : ""}`} />
    </div>
  );
}
