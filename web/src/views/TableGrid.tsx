import { useEffect, useMemo, useState } from "react";
import { api, type GraphNode, type RowCells } from "../api";
import type { ViewProps } from "./MapView";
import { TableCell } from "./TableView";
import { LinePlot, bandOf, numberOf } from "./SweepChart";
import { label } from "../theme";
import { UNWRITTEN } from "./MapView";

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
      todo.push(...g.edges.filter((l) => l.from === k && l.components.includes("is_a")).map((l) => l.to));
    }
    return seen;
  }, [g.edges, head]);
  // A row's other parts say what's theirs to say, in their own tables.
  const columns = useMemo(() => {
    const free = (k: string) => !ws.cellInfo(k) && k !== "name";
    const cs = new Set(Object.keys(head?.cells ?? {}).filter(free));
    for (const r of rows) for (const [k, c] of Object.entries(r.cells)) if (free(k) && (c.source === "own" || (c.kind && lineage.has(c.kind)))) cs.add(k);
    // A table of choices: each row's choice, and the options, which the rows take from the table. The
    // table's own value is what it says about them, such as that no two are the same.
    const choices = [head, ...rows].some((n) => n?.cells.options);
    return [...(choices ? ["value", "options"] : []), ...[...cs].sort()];
  }, [head, rows, lineage, ws]);
  const numbered = rows.length > 0 && rows.every((r) => typeof r.row!.key === "number");
  // A numbered table goes on past its last written row, as far as it's asked: those rows are worked out
  // from the table, and nothing about them is saved.
  const first = numbered ? (rows[0].row!.key as number) : 0;
  const last = numbered ? (rows[rows.length - 1].row!.key as number) : 0;
  // Ten rows on from the first, to start with, so a table that only writes where it starts still shows.
  const [upTo, setUpTo] = useState<number | null>(null);
  const to = Math.min(Math.max(upTo ?? first + 10, last), first + 999);
  const run = useRows(ws, table, first, to, ws.world, numbered);
  const written = new Map(rows.map((r) => [r.id, r]));
  // Every row from the first to as far as asked, written or not, in order.
  const shown: { node: GraphNode; key: string | number; unwritten: boolean }[] = numbered
    ? (run ?? []).map((r) => ({ node: written.get(r.id) ?? { id: r.id, name: `${table}[${r.key}]`, components: [], cells: r.cells, unwritten: true }, key: r.key, unwritten: !r.written }))
    : rows.map((r) => ({ node: r, key: r.row!.key, unwritten: false }));

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
                <td key={c} className={`sheet-cell ${head.cells[c]?.formula !== undefined ? "formula-cell" : ""}`}>
                  <TableCell ws={ws} node={head} c={c} cell={head.cells[c]} written />
                </td>
              ))}
            </tr>
          )}
          {shown.map(({ node: r, key, unwritten }) => (
            <tr key={r.id} className={`${r.id === selected ? "selected" : ""} ${unwritten ? "implicit-row" : ""}`} onClick={() => !unwritten && onSelect(r.id)}>
              <td className="sheet-name" title={unwritten ? `Nobody wrote ${r.name}. It's worked out from ${table}, and isn't saved: add it to say something of its own.` : r.name}>
                {unwritten && <span className="unwritten-mark">{UNWRITTEN} </span>}
                {String(key)}
              </td>
              {columns.map((c) => {
                const cell = r.cells[c];
                // Taken from the table, or from another part of the same thing, rather than said here.
                const taken = cell && cell.source !== "own";
                return (
                  <td key={c} className={`sheet-cell ${taken ? "taken" : ""} ${cell?.error ? "err" : ""} ${ws.recentCells.has(`${r.id}|${c}`) ? "ran" : ""}`} title={taken && cell.kind ? `From ${ws.byId.get(cell.kind)?.name ?? "elsewhere"}` : undefined}>
                    <TableCell ws={ws} node={r} c={c} cell={cell} readOnly={unwritten} />
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      {numbered && (
        <div className="table-new small">
          <span className="muted">Show up to row</span>
          <input type="number" aria-label="Show rows up to" value={to} min={last} onChange={(e) => setUpTo(Number(e.target.value))} />
          <span className="muted">
            {UNWRITTEN} marks a row nobody wrote: it's worked out from {table}, and isn't saved.
          </span>
        </div>
      )}
      <NewRow ws={ws} table={table} rows={rows} />
      {numbered && <RowChart ws={ws} table={table} from={first} to={to} columns={columns} />}
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
  const decisions = rows.map((r) => g.edges.filter((l) => l.from === r.id && l.components.includes("alternative")).map((l) => l.to));
  const shared = decisions.length && decisions[0].length === 1 && decisions.every((d) => d.length === 1 && d[0] === decisions[0][0]) ? decisions[0][0] : null;
  const add = async () => {
    const k = (key ?? String(next)).trim();
    if (!k) return;
    try {
      await ws.apply([{ op: "add_node", id: "new:row", name: `${table}[${k}]`, ...(shared ? { links: [{ to: shared, component: "alternative" }] } : {}) }]);
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

/** A numbered table's rows over a range, in one world, whether or not anyone wrote them. */
function useRows(ws: ViewProps["ws"], table: string, from: number, to: number, world: Record<string, string>, wanted: boolean): RowCells[] | null {
  const [rows, setRows] = useState<RowCells[] | null>(null);
  const key = `${ws.view!.last_seq}|${table}|${from}|${to}|${JSON.stringify(world)}|${wanted}`;
  useEffect(() => {
    if (!wanted) return setRows(null);
    let alive = true;
    api
      .rows(table, from, to, world)
      .then((r) => alive && setRows(r))
      .catch(() => alive && setRows([]));
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  return rows;
}

/**
 * A column down the rows, written or not. If it waits on a choice, there's a line for each option, each
 * worked out with that option pinned, so a choice part way down shows where the lines part.
 */
function RowChart({ ws, table, from, to, columns }: { ws: ViewProps["ws"]; table: string; from: number; to: number; columns: string[] }) {
  const g = ws.view!;
  const vars = g.worlds.vars;
  const here = useRows(ws, table, from, to, ws.world, true);
  const numeric = columns.filter((c) => (here ?? []).some((r) => numberOf(r.cells[c], vars) !== null || r.cells[c]?.shown?.kind === "cond"));
  const [chosen, setChosen] = useState<string | null>(null);
  const column = numeric.includes(chosen ?? "") ? chosen! : numeric.find((c) => (here ?? []).some((r) => r.cells[c]?.formula)) ?? numeric[0];
  // The first choice still to be made that the column waits on.
  const choice = vars.find((v) => !v.uncertain && (here ?? []).some((r) => r.cells[column ?? ""]?.vars?.includes(v.id)));
  const [lines, setLines] = useState<{ name: string; rows: RowCells[] }[] | null>(null);
  const key = `${g.last_seq}|${JSON.stringify(ws.world)}|${choice?.id}|${table}|${from}|${to}`;
  useEffect(() => {
    if (!choice) return setLines(null);
    let alive = true;
    const options = choice.options.filter((o) => o.possible);
    Promise.all(options.map((o) => api.rows(table, from, to, { ...ws.world, [choice.id]: o.id }).then((rows) => ({ name: o.name, rows }))))
      .then((ls) => alive && setLines(ls))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!column || !here) return null;

  const xs = here.map((r) => r.key);
  const down = (rows: RowCells[]) => rows.map((r) => numberOf(r.cells[column], vars));
  const [names, ys] = choice ? [(lines ?? []).map((l) => l.name), lines ? lines.map((l) => down(l.rows)) : null] : [[label(column)], [down(here)]];
  // Where the column's uncertain, a band behind each line: where each row falls nine times in ten.
  // A plain number among them is a band of no width, so the band opens out from it rather than starting beside it.
  const spread = (rows: RowCells[]) => {
    const found = rows.map((r) => bandOf(r.cells[column], vars));
    if (!found.some((b) => b)) return found;
    return found.map((b, i) => {
      const plain = b ? null : numberOf(rows[i].cells[column], vars);
      return b ?? (plain === null ? null : { low: plain, high: plain, exact: true });
    });
  };
  const bands = choice ? (lines ? lines.map((l) => spread(l.rows)) : null) : [spread(here)];
  const banded = (bands ?? []).flat().filter((b) => b !== null);

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
        {banded.length > 0 && (
          <span className="muted">
            · the line is its mean, and the band where it falls nine times in ten{banded.every((b) => b!.exact) ? "" : banded.some((b) => b!.exact) ? ", paler where that was read from draws" : ", read from draws"}
          </span>
        )}
      </div>
      <LinePlot xs={xs} ys={ys} bands={bands} names={names} xName="Row" label={`${label(column)} down the rows${choice ? ` for each answer to ${choice.name}` : ""}`} />
    </div>
  );
}
