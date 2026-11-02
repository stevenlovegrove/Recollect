import { useEffect, useMemo, useState } from "react";
import { api, type EventView, type OpView } from "../api";
import type { Workspace } from "../workspace";
import type { ViewProps } from "./MapView";
import { NodeChip, valueText } from "../bits";
import { label } from "../theme";
import { askWhy } from "../Ask";

/** An op, in words. */
export function OpLine({ op, ws, onSelect, self, names }: { op: OpView; ws: Workspace; onSelect: (id: string) => void; self?: string; names?: Map<string, string> }) {
  const chip = (id: string | null) => (id && id !== self ? <NodeChip ws={ws} id={id} onSelect={onSelect} fallback={names?.get(id) ?? (names ? "a new node" : undefined)} /> : null);
  const what = op.what ? label(op.what) : "";
  switch (op.verb) {
    case "create":
      return <span className="op"><span className="verb add">create</span> {chip(op.node)}</span>;
    case "delete":
      return <span className="op"><span className="verb del">delete</span> {chip(op.node)}</span>;
    case "set":
      return (
        <span className="op">
          <span className="verb upd">set</span> {chip(op.node)} <b>{what}</b>
          {op.value !== null && op.value !== undefined && <code className="op-patch">{valueText(op.value)}</code>}
        </span>
      );
    case "set_rule":
      return (
        <span className="op">
          <span className="verb upd">set</span> {chip(op.node)} <b>{what}</b> <span className="muted">from</span> <code className="op-patch">{valueText(op.value)}</code>
        </span>
      );
    case "remove":
      return <span className="op"><span className="verb del">remove</span> {chip(op.node)} <b>{what}</b></span>;
    case "link":
      return (
        <span className="op">
          <span className="verb add">link</span> {chip(op.node)} <span className="rel">{what}</span> {chip(op.other)}
          {op.value !== null && op.value !== undefined && <code className="op-patch">{valueText(op.value)}</code>}
        </span>
      );
    case "unlink":
      return <span className="op"><span className="verb del">unlink</span> {chip(op.node)} <span className="rel">{what}</span> {chip(op.other)}</span>;
    case "set_default":
      return <span className="op"><span className="verb upd">default</span> <b>{what}</b></span>;
    case "remove_default":
      return <span className="op"><span className="verb del">default</span> <b>{what}</b></span>;
  }
}

const LANE = 16;
const DOT_Y = 16;
const LANE_COLORS = ["var(--accent)", "var(--c-goal)", "var(--c-task)", "var(--c-question)", "var(--c-value)", "var(--c-requires)"];
const laneX = (i: number) => LANE / 2 + i * LANE;
const laneColor = (i: number) => LANE_COLORS[i % LANE_COLORS.length];

/** Where one event sits in the tree: its lane, and which lanes pass by above and below it. */
type Row = { col: number; above: (number | null)[]; below: (number | null)[] };

/** Lay out the tree newest first, as `git log --graph` does. Each lane is waiting for one event. */
function layOut(events: EventView[]): { rows: Map<number, Row>; width: number } {
  const rows = new Map<number, Row>();
  const lanes: (number | null)[] = [];
  let widest = 1;
  for (const e of events) {
    const above = [...lanes];
    const waiting = lanes.flatMap((seq, i) => (seq === e.seq ? [i] : []));
    let col = waiting[0];
    if (col === undefined) {
      col = lanes.indexOf(null);
      if (col < 0) col = lanes.length;
    }
    // Branches that split from here end here.
    for (const i of waiting.slice(1)) lanes[i] = null;
    lanes[col] = e.parent;
    rows.set(e.seq, { col, above, below: [...lanes] });
    widest = Math.max(widest, lanes.length);
  }
  return { rows, width: widest * LANE };
}

function TreeCell({ row, seq, width, head, here }: { row: Row; seq: number; width: number; head: boolean; here: boolean }) {
  const x = laneX(row.col);
  return (
    <svg className="history-graph" width={width} aria-hidden="true">
      {row.above.map((waiting, i) =>
        waiting === null ? null : waiting === seq ? (
          <path key={`a${i}`} d={`M ${laneX(i)} 0 C ${laneX(i)} ${DOT_Y / 2}, ${x} ${DOT_Y / 2}, ${x} ${DOT_Y}`} fill="none" stroke={laneColor(i)} strokeWidth="2" />
        ) : (
          <line key={`a${i}`} x1={laneX(i)} y1={0} x2={laneX(i)} y2={DOT_Y} stroke={laneColor(i)} strokeWidth="2" />
        ),
      )}
      {row.below.map((waiting, i) => (waiting === null ? null : <line key={`b${i}`} x1={laneX(i)} y1={DOT_Y} x2={laneX(i)} y2="100%" stroke={laneColor(i)} strokeWidth="2" />))}
      <circle cx={x} cy={DOT_Y} r={head ? 6 : 4} fill={here ? laneColor(row.col) : "var(--bg)"} stroke={laneColor(row.col)} strokeWidth="2" />
    </svg>
  );
}

/** The log, as the tree it is: who changed what, when and why, on every branch. */
export function HistoryView({ ws, onSelect }: ViewProps) {
  const [events, setEvents] = useState<EventView[]>([]);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [err, setErr] = useState<string | null>(null);
  const seq = ws.view!.last_seq;
  useEffect(() => {
    api.history().then(setEvents).catch((e) => setErr(String(e)));
  }, [seq]);

  const undo = async (e: EventView) => {
    const why = await askWhy(`Undo “${e.why || `event ${e.seq}`}”?`, "Undo");
    if (why === null) return;
    setErr(null);
    api.undo(e.seq, why).catch((x) => setErr(x instanceof Error ? x.message : String(x)));
  };
  const goTo = (e: EventView) => {
    setErr(null);
    ws.checkout(e.seq, `went to event ${e.seq}`).catch((x) => setErr(x instanceof Error ? x.message : String(x)));
  };

  const { rows, width } = useMemo(() => layOut(events), [events]);

  return (
    <div className="scroll pad">
      <div className="map-toolbar flat">
        <span className="muted">Every change, newest first. Going back to an event and changing something starts a new branch, and nothing is ever lost.</span>
      </div>
      {err && <div className="error">{err}</div>}
      <ol className="history">
        {events.map((e) => {
          const expanded = open.has(e.seq) || e.ops.length <= 3;
          const row = rows.get(e.seq);
          const head = e.seq === seq;
          return (
            <li key={e.seq} className={`${head ? "looking" : ""} ${e.here ? "" : "elsewhere"}`} style={{ paddingLeft: width + 8 }}>
              {row && <TreeCell row={row} seq={e.seq} width={width} head={head} here={e.here} />}
              <div className="history-head">
                <span className="history-seq">#{e.seq}</span>
                <b>{e.author}</b>
                <span className="muted small">{new Date(e.at).toLocaleString()}</span>
                <span className="spacer" />
                {head ? (
                  <span className="history-here small">you're here</span>
                ) : (
                  <button className="link" title="Everything goes to how it was here, and the next change follows from here" onClick={() => goTo(e)}>
                    {e.here ? "go back here" : "go to this"}
                  </button>
                )}
                {e.here && (
                  <button className="link" onClick={() => undo(e)}>
                    undo…
                  </button>
                )}
              </div>
              {e.why && <div className="history-why">{e.why}</div>}
              {expanded ? (
                <ul className="history-ops">
                  {e.ops.map((op, i) => (
                    <li key={i}>
                      <OpLine op={op} ws={ws} onSelect={onSelect} />
                    </li>
                  ))}
                </ul>
              ) : (
                <button className="link small" onClick={() => setOpen(new Set([...open, e.seq]))}>
                  show {e.ops.length} changes
                </button>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
