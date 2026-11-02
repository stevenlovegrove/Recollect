import { useMemo, useState } from "react";
import type { Cell, GraphNode } from "../api";
import type { ViewProps } from "./MapView";
import { Truth, cellText, reading } from "../bits";
import { DdItem, Dropdown } from "../Dropdown";
import { label, nodeColor } from "../theme";

/** Rows are nodes and columns are components, as they really are. Typed-in numbers edit in place. */
export function TableView({ ws, selected, onSelect }: ViewProps) {
  const g = ws.view!;
  const all = useMemo(() => {
    const present = new Set(g.nodes.flatMap((n) => Object.keys(n.cells)));
    present.delete("name");
    present.delete("body");
    return g.components.filter((c) => c.on === "node" && !c.quiet && present.has(c.name)).map((c) => c.name);
  }, [g.nodes, g.components]);
  // What a card would show, and what's typed in. Everything else is a click away.
  const preferred = useMemo(() => {
    const summaries = g.components.filter((c) => c.summary && all.includes(c.name)).map((c) => c.name);
    return summaries.length ? summaries : all.slice(0, 4);
  }, [g.components, all]);
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
        <Dropdown label={<>Columns {columns.length}/{all.length}</>}>
          {all.map((c) => (
            <DdItem key={c} checked={columns.includes(c)} onChange={() => toggle(c)} title={ws.info(c)?.about}>
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
              <th key={c} title={ws.info(c)?.about}>
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
                <td key={c} className={`sheet-cell ${n.cells[c]?.error ? "err" : ""} ${ws.recentCells.has(`${n.id}|${c}`) ? "ran" : ""} ${n.cells[c]?.source === "rule" ? "formula-cell" : ""}`}>
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

function TableCell({ ws, node, c, cell }: { ws: ViewProps["ws"]; node: GraphNode; c: string; cell: Cell | undefined }) {
  if (!cell) return null;
  const nameOf = (s: string) => ws.byId.get(s)?.name ?? s;
  if (cell.error) return <span title={cell.error}>#ERROR</span>;
  if (cell.source === "stored" && typeof cell.value === "number") {
    return <NumberCell key={cell.value} value={cell.value} onCommit={(value) => ws.apply([{ op: "set", id: node.id, component: c, value }]).catch((e) => alert(String(e)))} />;
  }
  const r = reading(cell);
  if (r) return <Truth component={c} r={r} />;
  return <span title={cell.rule}>{cellText(cell, nameOf)}</span>;
}

function NumberCell({ value, onCommit }: { value: number; onCommit: (v: number) => void }) {
  const [text, setText] = useState(String(value));
  return (
    <input
      className="sheet-input"
      type="number"
      step="any"
      value={text}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => text !== "" && Number(text) !== value && onCommit(Number(text))}
      onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
    />
  );
}
