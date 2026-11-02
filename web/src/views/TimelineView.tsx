import { useMemo } from "react";
import type { GraphNode } from "../api";
import type { ViewProps } from "./MapView";
import { casesOf, reading } from "../bits";

type Row = { node: GraphNode; depth: number; again: boolean };

type Span = { start: number | null; finish: number | null; slack: number | null };

/** When it happens, in each world still in play that needs it. */
const spans = (n: GraphNode) => casesOf(n.cells.dates).filter((v): v is Span => v !== null);
const known = (values: (number | null)[]) => values.filter((v): v is number => v !== null);

/**
 * Dates, as a chart. With decisions open a date can be several: the solid bar is what's true in
 * every world still in play, and the hatching runs out to the latest it could be.
 */
export function TimelineView({ ws, selected, onSelect }: ViewProps) {
  const g = ws.view!;

  // The structure, top down, so a summary sits above what it adds up.
  const rows = useMemo(() => {
    const kids = new Map<string, string[]>();
    const hasParent = new Set<string>();
    for (const l of g.links) {
      if (ws.arranging.includes(l.kind)) {
        kids.set(l.to, [...(kids.get(l.to) ?? []), l.from]);
        hasParent.add(l.from);
      }
    }
    const out: Row[] = [];
    const seen = new Set<string>();
    // Only what's in the plan: work, gaps, and what they add up to.
    const inPlan = (id: string): boolean => {
      const n = ws.byId.get(id);
      return !!n && (!!n.cells.task || !!n.cells.unknown || (kids.get(id) ?? []).some(inPlan));
    };
    const walk = (id: string, depth: number) => {
      const node = ws.byId.get(id);
      if (!node || !inPlan(id)) return;
      const again = seen.has(id);
      seen.add(id);
      out.push({ node, depth, again });
      if (!again) for (const k of kids.get(id) ?? []) walk(k, depth + 1);
    };
    for (const n of g.nodes) if (!hasParent.has(n.id)) walk(n.id, 0);
    return out;
  }, [g.nodes, g.links, ws.arranging, ws.byId]);

  const finishes = rows.flatMap((r) => known(spans(r.node).map((s) => s.finish)));
  const days = Math.max(1, Math.ceil(Math.max(0, ...finishes)));
  const unknownEnd = rows.some((r) => spans(r.node).some((s) => s.finish === null));

  const W = 880;
  const L = 280;
  const RH = 28;
  const top = 26;
  const x = (d: number) => L + (d / days) * (W - L - 56);
  const height = top + rows.length * RH + 8;
  const every = days > 40 ? 10 : days > 20 ? 5 : 1;
  const axis = Array.from({ length: Math.floor(days / every) + 1 }, (_, i) => i * every);

  return (
    <div className="scroll pad timeline">
      <div className="map-toolbar flat">
        <span className="muted">
          Days from the start. {Object.keys(ws.world).length === 0 && g.worlds.vars.length > 0 && "Decisions are open, so hatching shows how far a date could move, and a pale bar is work only some worlds need."}
          {unknownEnd && " Something isn't specified in a world still in play, so the end can't be dated there."}
        </span>
      </div>
      <svg viewBox={`0 0 ${W} ${height}`} width={W} className="timeline-chart" role="img" aria-label="When each piece of work happens">
        <defs>
          <pattern id="hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <rect width="6" height="6" className="hatch-bg" />
            <line x1="0" y1="0" x2="0" y2="6" className="hatch-line" />
          </pattern>
        </defs>
        {axis.map((d) => (
          <g key={d}>
            <line x1={x(d)} x2={x(d)} y1={top} y2={height - 6} className="axis-line" />
            <text x={x(d)} y={top - 8} textAnchor="middle" className="axis-text">
              {d}
            </text>
          </g>
        ))}
        {rows.map((r, i) => {
          const y = top + i * RH + 4;
          const n = r.node;
          const here = spans(n);
          const starts = known(here.map((s) => s.start));
          const ends = known(here.map((s) => s.finish));
          const slack = known(here.map((s) => s.slack));
          const isSummary = !n.cells.task || g.links.some((l) => ws.arranging.includes(l.kind) && l.to === n.id);
          const out = !here.length;
          const someWorldsOnly = !out && !reading(n.cells.need)?.always;
          const done = n.cells.done !== undefined;
          return (
            <g key={`${n.id}-${i}`} className={`timeline-row ${selected === n.id ? "selected" : ""}`} onClick={() => onSelect(n.id)}>
              <rect x={0} y={y - 4} width={W} height={RH} className="row-hit" />
              <text x={8 + r.depth * 14} y={y + 14} className={`row-name ${out ? "out" : ""} ${isSummary ? "summary" : ""}`}>
                {n.name.length > 34 - r.depth * 2 ? `${n.name.slice(0, 33 - r.depth * 2)}…` : n.name}
              </text>
              {r.again && !out && <text x={L + 4} y={y + 14} className="row-note">shared: the same work as above</text>}
              {!r.again && out && <text x={L + 4} y={y + 14} className="row-note">not needed here</text>}
              {!r.again && !out && !ends.length && <text x={L + 4} y={y + 14} className="row-note err">can't be dated: something isn't specified</text>}
              {!r.again && !out && ends.length > 0 && starts.length > 0 && (() => {
                const start = Math.min(...starts);
                const lastStart = Math.max(...starts);
                const firstEnd = Math.min(...ends);
                const lastEnd = Math.max(...ends);
                if (isSummary) {
                  const a = x(start);
                  const b = Math.max(x(lastEnd), a + 2);
                  return <path d={`M${a} ${y + 15} V${y + 7} H${b} V${y + 15}`} className="summary-bar" />;
                }
                return (
                  <>
                    {lastEnd > firstEnd && <rect x={x(firstEnd)} y={y + 1} width={x(lastEnd) - x(firstEnd)} height={18} fill="url(#hatch)" />}
                    {lastStart > start && <rect x={x(start)} y={y + 1} width={x(lastStart) - x(start)} height={18} fill="url(#hatch)" />}
                    {firstEnd > lastStart && <rect x={x(lastStart)} y={y + 1} width={Math.max(2, x(firstEnd) - x(lastStart))} height={18} rx={3} className={`${done ? "bar-done" : "bar-work"} ${someWorldsOnly ? "contingent" : ""}`} />}
                    {slack.length > 0 && Math.min(...slack) > 0 && <rect x={x(lastEnd)} y={y + 7} width={x(lastEnd + Math.min(...slack)) - x(lastEnd)} height={6} className="bar-slack"><title>Could slip by at least {Math.min(...slack)} without moving the end</title></rect>}
                    {slack.length > 0 && Math.max(...slack) === 0 && <circle cx={x(lastEnd) + 8} cy={y + 10} r={3} className="dot-critical"><title>Critical: any slip moves the end</title></circle>}
                  </>
                );
              })()}
            </g>
          );
        })}
      </svg>
      <div className="legend">
        <span><i className="swatch bar-work" /> work</span>
        <span><i className="swatch bar-work contingent" /> only in some worlds</span>
        <span><i className="swatch bar-done" /> finished</span>
        <span><i className="swatch hatch" /> depends on what's decided</span>
        <span><i className="swatch bar-slack" /> slack: room to slip</span>
        <span><i className="swatch dot" /> critical: any slip moves the end</span>
      </div>
    </div>
  );
}
