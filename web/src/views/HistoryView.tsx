import { useEffect, useState } from "react";
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
          <span className="verb upd">set</span> {chip(op.node)}
          {op.other && (
            <>
              {" "}
              <span className="muted">→</span> {chip(op.other)}
            </>
          )}{" "}
          <b>{what}</b>
          {op.value !== null && op.value !== undefined && <code className="op-patch">{op.formula ? `=${valueText(op.value)}` : valueText(op.value)}</code>}
        </span>
      );
    case "clear":
      return (
        <span className="op">
          <span className="verb del">clear</span> {chip(op.node)}
          {op.other && (
            <>
              {" "}
              <span className="muted">→</span> {chip(op.other)}
            </>
          )}{" "}
          <b>{what}</b>
        </span>
      );
    case "add":
      return <span className="op"><span className="verb add">add</span> <b>{what}</b>{op.node !== self && <> <span className="muted">to</span> {chip(op.node)}</>}</span>;
    case "remove":
      return <span className="op"><span className="verb del">remove</span> <b>{what}</b>{op.node !== self && <> <span className="muted">from</span> {chip(op.node)}</>}</span>;
    case "link":
      return <span className="op"><span className="verb add">link</span> {chip(op.node)} <span className="rel">{what}</span> {chip(op.other)}</span>;
    case "unlink":
      return <span className="op"><span className="verb del">unlink</span> {chip(op.node)} <span className="rel">{what}</span> {chip(op.other)}</span>;
  }
}

/** What's changed since the workspace was opened, newest first: who changed what, when and why. */
export function HistoryView({ ws, onSelect }: ViewProps) {
  const [events, setEvents] = useState<EventView[]>([]);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [err, setErr] = useState<string | null>(null);
  const v = ws.view!;
  const seq = v.last_seq;
  useEffect(() => {
    api.history().then(setEvents).catch((e) => setErr(String(e)));
  }, [seq]);

  const run = (f: () => Promise<unknown>) => {
    setErr(null);
    f().catch((x) => setErr(x instanceof Error ? x.message : String(x)));
  };
  const undoOne = async (e: EventView) => {
    const why = await askWhy(`Undo “${e.why || `event ${e.seq}`}”?`, "Undo");
    if (why !== null) run(() => api.undo(e.seq, why).then(() => ws.refresh()));
  };

  return (
    <div className="scroll pad">
      <div className="map-toolbar flat">
        <button className="key-host" disabled={v.undo === null} onClick={() => run(ws.undo)} title="Undo (⌘Z)">
          Undo
        </button>
        <button className="key-host" disabled={v.redo === null} onClick={() => run(ws.redo)} title="Redo (⇧⌘Z)">
          Redo
        </button>
        <span className="muted">Every change since this was opened, newest first. Undoing is a change too, so it can be redone, and any one change can be taken back on its own.</span>
      </div>
      {err && <div className="error">{err}</div>}
      {events.length === 0 && <p className="muted">Nothing's changed yet.</p>}
      <ol className="history">
        {events.map((e) => {
          const expanded = open.has(e.seq) || e.ops.length <= 3;
          return (
            <li key={e.seq} className={e.seq === v.undo ? "looking" : ""}>
              <div className="history-head">
                <span className="history-seq">#{e.seq}</span>
                <b>{e.author}</b>
                <span className="muted small">{new Date(e.at).toLocaleString()}</span>
                {e.reverts !== null && <span className="muted small">takes back #{e.reverts}</span>}
                <span className="spacer" />
                <button className="link" onClick={() => undoOne(e)}>
                  undo this…
                </button>
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
