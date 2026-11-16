import { useEffect, useMemo, useState } from "react";
import { api, type GraphNode } from "../api";
import type { Workspace } from "../workspace";
import { LinePlot, bandOf, numberOf, type Band } from "./SweepChart";

/** What a chart node says to plot, beside what it labels: see `Chart` in `formulas.rs`. */
type Spec = { from: number; to: number; points?: { table: string; x: string; y: string } };

const STEPS = 40;

/**
 * A chart node, drawn: what it labels `y`, read as what it labels `x` goes from one number to another,
 * each with the band it falls in nine times in ten, and a table's two columns as points behind them.
 * Every point on a curve is what you'd see with that value typed in, worked out on a copy, so the chart
 * can't disagree with the cells.
 */
export function ChartView({ ws, node }: { ws: Workspace; node: GraphNode }) {
  const view = ws.view!;
  const spec = node.cells.chart?.value as Spec | undefined;
  // What it plots, by the labels it has: one called x, and any called y, in order.
  const labelled = view.edges.filter((e) => e.from === node.id && e.components.includes("label"));
  const x = labelled.find((e) => e.cells.called?.value === "x")?.to;
  const ys = labelled.filter((e) => e.cells.called?.value === "y").map((e) => e.to);
  const nameOf = (id: string) => ws.byId.get(id)?.name ?? id;
  const xs = useMemo(() => (spec ? Array.from({ length: STEPS + 1 }, (_, i) => spec.from + ((spec.to - spec.from) * i) / STEPS) : []), [spec?.from, spec?.to]); // eslint-disable-line react-hooks/exhaustive-deps
  const [curves, setCurves] = useState<{ ys: (number | null)[][]; bands: (Band | null)[][] } | null>(null);
  const key = `${view.last_seq}|${JSON.stringify(ws.world)}|${x}|${ys.join()}|${xs.join()}`;

  useEffect(() => {
    if (!x || !ys.length || !xs.length) return;
    let alive = true;
    setCurves(null);
    api
      .sweep({ node: x, cell: "value" }, xs, ys.map((y) => ({ node: y, cell: "value" })), ws.world)
      .then((swept) => {
        if (!alive) return;
        const vars = ws.view!.worlds.vars;
        setCurves({
          ys: ys.map((_, j) => swept.map((p) => numberOf(p.cells[j], vars))),
          bands: ys.map((_, j) => swept.map((p) => bandOf(p.cells[j], vars))),
        });
      })
      .catch(() => alive && setCurves({ ys: [], bands: [] }));
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps

  // The table's rows as points: those with a number in both columns.
  const points = useMemo(() => {
    const p = spec?.points;
    if (!p) return [];
    const vars = view.worlds.vars;
    return view.nodes
      .filter((n) => n.row?.table === p.table)
      .map((n) => ({ x: numberOf(n.cells[p.x], vars), y: numberOf(n.cells[p.y], vars), name: n.name }))
      .filter((q): q is { x: number; y: number; name: string } => q.x !== null && q.y !== null);
  }, [view, spec?.points]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!spec) return null;
  if (!x || !ys.length)
    return <div className="muted small chart-missing">Label what to vary x, and what to plot y: one x, and as many y as you like. x has to be a number that's typed in.</div>;
  const xName = nameOf(x);
  return (
    <div className="sweep chart-view">
      <LinePlot
        xs={xs}
        ys={curves?.ys ?? null}
        bands={curves?.bands ?? null}
        names={ys.map(nameOf)}
        xName={xName}
        points={points}
        pointsName={spec.points ? `${spec.points.table}: ${spec.points.y} against ${spec.points.x}` : undefined}
        label={`${ys.map(nameOf).join(" and ")} as ${xName} goes from ${spec.from} to ${spec.to}${spec.points ? `, with ${spec.points.table} as points` : ""}`}
      />
      {curves === null && <div className="muted small">Working it out…</div>}
      {curves !== null && !curves.ys.flat().some((v) => v !== null) && <div className="muted small">None of these change with {xName}, or they still depend on a choice.</div>}
    </div>
  );
}
