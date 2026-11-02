import type { Standing } from "../api";
import type { ViewProps } from "./MapView";
import { Formula, NodeChip, TickBox } from "../bits";

const COLUMNS: { standing: Standing; title: string; hint: string }[] = [
  { standing: "now", title: "Can start now", hint: "Safe whichever named option is taken" },
  { standing: "depends", title: "Depends on a decision", hint: "Could start, but only some worlds need it" },
  { standing: "waiting", title: "Waiting", hint: "Needed, and held up by something" },
  { standing: "finished", title: "Finished", hint: "" },
  { standing: "not_needed", title: "Not needed", hint: "No world still in play needs it" },
];

/** Work that can be picked up, and where each piece stands among the worlds in play. */
export function WorkView({ ws, selected, onSelect }: ViewProps) {
  const items = ws.plan.work;
  if (!items.length) return <p className="muted pad">No tasks yet. Give a node with no parts a ▸ Task to put it here.</p>;
  return (
    <div className="plan">
      <div className="columns">
        {COLUMNS.map((c) => {
          const col = items.filter((i) => i.standing === c.standing);
          return (
            <div key={c.standing} className={`column col-${c.standing}`}>
              <h3 title={c.hint}>
                {c.title} <span className="muted">{col.length}</span>
              </h3>
              {col.map((i) => (
                <div key={i.id} className={`plan-card ${i.id === selected ? "selected" : ""} ${ws.recent.has(i.id) ? "recent" : ""}`} onClick={() => onSelect(i.id)}>
                  <div className="plan-title">
                    <TickBox ws={ws} id={i.id} c="done" /> {i.name}
                    {i.estimate !== null && <span className="muted small"> · {i.estimate}d</span>}
                  </div>
                  {!i.need.always && !i.need.never && (
                    <div className="plan-row">
                      <span className="muted small">needed when</span> <Formula r={i.need} />
                    </div>
                  )}
                  {i.blockers.length > 0 && (
                    <div className="plan-row" onClick={(e) => e.stopPropagation()}>
                      <span className="muted small">waits for</span>
                      {i.blockers.map((b) => (
                        <span key={b.id} className="named-item">
                          <NodeChip ws={ws} id={b.id} onSelect={onSelect} />
                          {b.when !== "always" && <Formula r={b.when} />}
                        </span>
                      ))}
                    </div>
                  )}
                  {i.scope && <div className="plan-row muted small">in {i.scope.name}</div>}
                </div>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
