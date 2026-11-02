import { useState } from "react";
import { api, type Proposal } from "../api";
import type { ViewProps } from "./MapView";
import type { Workspace } from "../workspace";
import { renderMarkdown } from "../markdown";
import { OpLine } from "./HistoryView";

export function InboxView({ ws, onSelect }: ViewProps) {
  if (ws.proposals.length === 0) {
    return (
      <div className="pad empty">
        <h3>Inbox zero</h3>
        <p className="muted">
          Change-sets Claude submits with the <code>propose</code> tool land here for review.
        </p>
      </div>
    );
  }
  return (
    <div className="scroll pad">
      {ws.proposals.map((p) => (
        <ProposalCard key={p.id} p={p} ws={ws} onSelect={onSelect} />
      ))}
    </div>
  );
}

function ProposalCard({ p, ws, onSelect }: { p: Proposal; ws: Workspace; onSelect: (id: string) => void }) {
  const [checked, setChecked] = useState<Set<number>>(() => new Set(p.ops.map((_, i) => i)));
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const act = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setErr(null);
    try {
      await f();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const all = checked.size === p.ops.length;
  // Nodes the proposal itself makes aren't in the graph yet, so the proposal says what they're called.
  const names = new Map(p.ops.filter((o) => o.verb === "set" && o.what === "name" && o.node).map((o) => [o.node!, String(o.value)]));

  return (
    <article className="proposal">
      <header>
        <h3>{p.title}</h3>
        <span className="muted small">
          {p.author === "claude" ? "✦ Claude" : p.author} · {new Date(p.created).toLocaleString()} · {p.ops.length} change{p.ops.length > 1 ? "s" : ""}
        </span>
      </header>
      {p.rationale && <div className="markdown rationale" dangerouslySetInnerHTML={{ __html: renderMarkdown(p.rationale) }} />}
      {p.error && <div className="error">It no longer applies: {p.error}</div>}
      <ol className="ops">
        {p.ops.map((op, i) => (
          <li key={i}>
            <input
              type="checkbox"
              checked={checked.has(i)}
              onChange={() => {
                const next = new Set(checked);
                if (next.has(i)) next.delete(i);
                else next.add(i);
                setChecked(next);
              }}
            />
            <OpLine op={op} ws={ws} onSelect={onSelect} names={names} />
          </li>
        ))}
      </ol>
      {err && <div className="error">{err}</div>}
      <footer className="row end">
        <button disabled={busy} onClick={() => act(() => api.reject(p.id))}>
          Reject
        </button>
        <button className="primary" disabled={busy || checked.size === 0} onClick={() => act(() => api.accept(p.id, all ? undefined : [...checked].sort((a, b) => a - b)))}>
          {all ? "Accept" : `Accept ${checked.size} of ${p.ops.length}`}
        </button>
      </footer>
    </article>
  );
}
