import { useEffect, useMemo, useState } from "react";
import { api, type Cell, type CellRef, type VarView } from "../api";
import type { Workspace } from "../workspace";
import { casesOf, valueText } from "../bits";

/** One line: an option, and where to read its value as the input changes. */
export type Line = { id: string; name: string; read: CellRef; world: Record<string, string> };

/** A number for a cell, averaged over what's uncertain. Over a choice still to be made there's no one number. */
export function numberOf(cell: Cell | null | undefined, vars: VarView[]): number | null {
  if (!cell || cell.error) return null;
  if ((cell.vars ?? []).some((id) => !vars.find((v) => v.id === id)?.uncertain)) return null;
  const s = cell.shown;
  if (s?.kind === "cond") {
    const total = s.cases.reduce((t, c) => t + (typeof c.value === "number" ? c.when.prob : 0), 0);
    if (!total || s.cases.some((c) => typeof c.value !== "number")) return null;
    return s.cases.reduce((t, c) => t + (c.value as number) * c.when.prob, 0) / total;
  }
  return typeof cell.value === "number" ? cell.value : null;
}

/** The numbers worth trying in place of what's there: around everything it could be. */
function around(cell: Cell | undefined): [number, number] {
  const ns = casesOf(cell).filter((v): v is number => typeof v === "number");
  if (!ns.length) return [0, 1];
  const [lo, hi] = [Math.min(...ns), Math.max(...ns)];
  if (lo === hi) return lo === 0 ? [0, 1] : [Math.min(lo * 0.5, lo * 1.5), Math.max(lo * 0.5, lo * 1.5)];
  const pad = (hi - lo) / 2;
  return [lo >= 0 ? Math.max(0, lo - pad) : lo - pad, hi + pad];
}

const STEPS = 24;

/**
 * How each line moves as one input goes from a little to a lot. Every point is what you'd see with that
 * value typed in, worked out on a copy, so the chart can't disagree with the cells.
 */
export function SweepChart({ ws, lines, inputs, title }: { ws: Workspace; lines: Line[]; inputs: { node: string; name: string }[]; title: string }) {
  const [input, setInput] = useState(inputs[0]?.node ?? "");
  const cell = ws.byId.get(input)?.cells.value;
  const [lo, hi] = around(cell);
  const [range, setRange] = useState<{ for: string; lo: number; hi: number } | null>(null);
  const [from, to] = range?.for === input ? [range.lo, range.hi] : [lo, hi];
  const xs = useMemo(() => Array.from({ length: STEPS + 1 }, (_, i) => from + ((to - from) * i) / STEPS), [from, to]);
  const [ys, setYs] = useState<(number | null)[][] | null>(null);
  const key = `${ws.view?.last_seq}|${input}|${from}|${to}|${lines.map((l) => l.id + l.read.node + l.read.cell + JSON.stringify(l.world)).join()}`;

  useEffect(() => {
    if (!input) return;
    let alive = true;
    setYs(null);
    const vars = ws.view!.worlds.vars;
    Promise.all(lines.map((l) => api.sweep({ node: input, cell: "value" }, xs, [l.read], l.world).then((r) => r.map((p) => numberOf(p.cells[0], vars)))))
      .then((r) => alive && setYs(r))
      .catch(() => alive && setYs([]));
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps

  const inputName = inputs.find((i) => i.node === input)?.name ?? "";
  const all = (ys ?? []).flat().filter((v): v is number => v !== null);

  return (
    <div className="sweep">
      <div className="sweep-controls small">
        <span className="muted">{title} as</span>
        <select value={input} onChange={(e) => setInput(e.target.value)} aria-label="What to vary">
          {inputs.map((i) => (
            <option key={i.node} value={i.node}>
              {i.name}
            </option>
          ))}
        </select>
        <span className="muted">goes from</span>
        <input type="number" aria-label="From" value={+from.toPrecision(6)} onChange={(e) => setRange({ for: input, lo: Number(e.target.value), hi: to })} />
        <span className="muted">to</span>
        <input type="number" aria-label="To" value={+to.toPrecision(6)} onChange={(e) => setRange({ for: input, lo: from, hi: Number(e.target.value) })} />
      </div>
      <LinePlot xs={xs} ys={ys} names={lines.map((l) => l.name)} xName={inputName} label={`${title} for each option as ${inputName} goes from ${valueText(from)} to ${valueText(to)}`} />
      {ys === null && <div className="muted small">Working it out…</div>}
      {ys !== null && !all.length && <div className="muted small">Nothing here changes with {inputName}, or it still depends on a choice.</div>}
    </div>
  );
}

const [W, H, LEFT, RIGHT, TOP, BOTTOM] = [640, 240, 64, 112, 12, 30];

/**
 * Lines over a run of numbers, one per name, with each named at its end and a crosshair that reads them
 * all. A null is a gap in a line.
 */
export function LinePlot({ xs, ys, names, xName, label }: { xs: number[]; ys: (number | null)[][] | null; names: string[]; xName: string; label: string }) {
  const [hover, setHover] = useState<number | null>(null);
  const [from, to] = [xs[0] ?? 0, xs[xs.length - 1] ?? 1];
  const last = xs.length - 1;
  const all = (ys ?? []).flat().filter((v): v is number => v !== null);
  const [ylo, yhi] = all.length ? [Math.min(...all), Math.max(...all)] : [0, 1];
  const ypad = (yhi - ylo || Math.abs(yhi) || 1) * 0.08;
  const [y0, y1] = [ylo - ypad, yhi + ypad];
  const px = (x: number) => LEFT + ((x - from) / (to - from || 1)) * (W - LEFT - RIGHT);
  const py = (y: number) => TOP + (1 - (y - y0) / (y1 - y0 || 1)) * (H - TOP - BOTTOM);
  const ticks = [0, 1, 2, 3].map((i) => y0 + ((y1 - y0) * i) / 3);
  // Each line's name at its end, nudged apart where lines end close together.
  const ends = (ys ?? [])
    .map((row, li) => ({ li, y: [...row].reverse().find((v) => v !== null) }))
    .filter((e): e is { li: number; y: number } => e.y !== undefined && e.y !== null)
    .map((e) => ({ li: e.li, at: py(e.y) }))
    .sort((a, b) => a.at - b.at);
  for (let i = 1; i < ends.length; i++) ends[i].at = Math.max(ends[i].at, ends[i - 1].at + 16);

  return (
    <>
      <div className="sweep-legend small">
        {names.map((n, i) => (
          <span key={n + i}>
            <i className="sweep-swatch" style={{ background: `var(--series-${(i % 8) + 1})` }} /> {n}
          </span>
        ))}
      </div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="sweep-plot"
        role="img"
        aria-label={label}
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const x = ((e.clientX - r.left) / r.width) * W;
          const i = Math.round(((x - LEFT) / (W - LEFT - RIGHT)) * last);
          setHover(i >= 0 && i <= last ? i : null);
        }}
      >
        {ticks.map((t, i) => (
          <g key={i}>
            <line x1={LEFT} x2={W - RIGHT} y1={py(t)} y2={py(t)} className={i === 0 ? "sweep-axis" : "sweep-grid"} />
            <text x={LEFT - 6} y={py(t) + 4} textAnchor="end" className="sweep-tick">
              {valueText(+t.toPrecision(3))}
            </text>
          </g>
        ))}
        {[...new Set([0, Math.round(last / 2), last])].map((i) => (
          <text key={i} x={px(xs[i])} y={H - 10} textAnchor={i === 0 ? "start" : i === last ? "end" : "middle"} className="sweep-tick">
            {valueText(+xs[i].toPrecision(3))}
          </text>
        ))}
        {ys?.map((row, li) => {
          const pts = row.map((y, i) => (y === null ? null : `${px(xs[i])},${py(y)}`));
          const d = pts.reduce<string>((acc, p, i) => (p === null ? acc : `${acc}${i === 0 || pts[i - 1] === null ? "M" : "L"}${p}`), "");
          return <path key={names[li]} d={d} fill="none" stroke={`var(--series-${(li % 8) + 1})`} strokeWidth={2} strokeLinejoin="round" />;
        })}
        {ends.map((e) => (
          <text key={e.li} x={W - RIGHT + 6} y={e.at + 4} className="sweep-label">
            {names[e.li]}
          </text>
        ))}
        {hover !== null && ys && (
          <g pointerEvents="none">
            <line x1={px(xs[hover])} x2={px(xs[hover])} y1={TOP} y2={H - BOTTOM} className="sweep-cross" />
            {ys.map((row, li) => row[hover] !== null && <circle key={li} cx={px(xs[hover])} cy={py(row[hover]!)} r={4} fill={`var(--series-${(li % 8) + 1})`} stroke="var(--panel)" strokeWidth={2} />)}
          </g>
        )}
        <rect x={LEFT} y={TOP} width={W - LEFT - RIGHT} height={H - TOP - BOTTOM} fill="transparent" />
      </svg>
      {hover !== null && ys && (
        <div className="sweep-tip small">
          <b>
            {xName} {valueText(+xs[hover].toPrecision(4))}
          </b>
          {names.map((n, li) => (
            <span key={n + li}>
              <i className="sweep-swatch" style={{ background: `var(--series-${(li % 8) + 1})` }} /> {n} {ys[li]?.[hover] === null || ys[li]?.[hover] === undefined ? "–" : valueText(+ys[li][hover]!.toFixed(2))}
            </span>
          ))}
        </div>
      )}
    </>
  );
}
