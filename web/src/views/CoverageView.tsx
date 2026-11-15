import { useEffect, useMemo, useState } from "react";
import { api, type Coverage } from "../api";
import type { ViewProps } from "./MapView";

/**
 * What realises each concept in each later layer: a row for each instance of a kind, a column for each
 * layer, and in each cell the formats or traits that realise it there. A blank is a gap: what's still only
 * an idea in that layer, which is the to-do list for going from one layer to the next.
 */
export function CoverageView({ ws, onSelect }: ViewProps) {
  const view = ws.view!;
  // The kinds there are: what anything links to by is_a. The one realised most is the likeliest to want.
  const kinds = useMemo(() => {
    const ofKind = new Map<string, number>();
    const realised = new Set(view.edges.filter((e) => e.components.includes("realises")).map((e) => e.to));
    for (const e of view.edges.filter((e) => e.components.includes("is_a"))) {
      ofKind.set(e.to, (ofKind.get(e.to) ?? 0) + (realised.has(e.from) ? 10 : 1));
    }
    return [...ofKind].sort((a, b) => b[1] - a[1]).map(([id]) => ({ id, name: ws.byId.get(id)?.name ?? id }));
  }, [view.edges, ws.byId]);
  const [kind, setKind] = useState<string | null>(null);
  const chosen = kind ?? kinds[0]?.id ?? null;
  const [coverage, setCoverage] = useState<Coverage | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!chosen) return;
    let stale = false;
    api
      .read({ coverage: { kind: chosen } })
      .then((r) => !stale && (setCoverage(r.coverage ?? null), setError(null)))
      .catch((e) => !stale && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      stale = true;
    };
  }, [chosen, view.last_seq]);

  if (!kinds.length) return <div className="scroll pad muted">Nothing is a kind of anything yet, so there's nothing to cover.</div>;
  const gaps = coverage?.rows.reduce((n, r) => n + r.gaps.length, 0) ?? 0;

  return (
    <div className="scroll pad coverage">
      <div className="map-toolbar flat">
        <label className="muted small">
          Each{" "}
          <select value={chosen ?? ""} onChange={(e) => setKind(e.target.value)}>
            {kinds.map((k) => (
              <option key={k.id} value={k.id}>
                {k.name}
              </option>
            ))}
          </select>{" "}
          and what realises it in each later layer.
        </label>
        {coverage && <span className="muted small">{gaps === 0 ? "No gaps." : `${gaps} ${gaps === 1 ? "gap" : "gaps"}: still only an idea there.`}</span>}
      </div>
      {error && <div className="error">{error}</div>}
      {coverage && (
        coverage.columns.length === 0 ? (
          <p className="muted">Nothing realises a {coverage.kind} yet. Link a format or a trait to one with a <code>realises</code> edge.</p>
        ) : (
          <table className="grid">
            <thead>
              <tr>
                <th>{coverage.kind}</th>
                {coverage.columns.map((c) => (
                  <th key={c}>{c}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {coverage.rows.map((r) => (
                <tr key={r.id}>
                  <td>
                    <button className="link" onClick={() => onSelect(r.id)}>
                      {r.name}
                    </button>
                  </td>
                  {coverage.columns.map((c) => (
                    <td key={c} className={r.by[c] ? "" : "gap"}>
                      {(r.by[c] ?? []).map((n) => (
                        <button key={n.id} className="link" onClick={() => onSelect(n.id)}>
                          {n.name}
                        </button>
                      ))}
                      {!r.by[c] && <span className="muted small">gap</span>}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )
      )}
    </div>
  );
}
