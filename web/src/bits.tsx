import { useMemo, useState } from "react";
import type { Cell, Chance, Dist, Edge, Estimate, GraphNode, Op, Reading, Shown } from "./api";
import { bracketRuns, partText } from "./brackets";
import type { Workspace } from "./workspace";
import { badgesOf, compColor, isDropped, isNote, label, nodeColor } from "./theme";

export function NodeChip({ ws, id, onSelect, fallback }: { ws: Workspace; id: string; onSelect: (id: string) => void; fallback?: string }) {
  const n = ws.byId.get(id);
  if (!n && fallback) return <span className="chip new">{fallback}</span>;
  return (
    <button className={`chip ${n ? (isDropped(n) ? "dead" : "") : "missing"}`} onClick={() => n && onSelect(id)} title={n ? badgesOf(n).map(label).join(", ") || (isNote(n) ? "note" : "") : "not in this graph"}>
      <span className="chip-dot" style={{ background: n ? nodeColor(n) : "var(--danger)" }} />
      {n?.name ?? `${id.slice(-6)}`}
    </button>
  );
}

export function CompBadges({ ws, n }: { ws: Workspace; n: GraphNode }) {
  return (
    <span className="badges">
      {badgesOf(n).map((c) => (
        <CompBadge key={c} ws={ws} c={c} />
      ))}
    </span>
  );
}

export function CompBadge({ ws, c, onClick }: { ws: Workspace; c: string; onClick?: () => void }) {
  const icon = ws.info(c)?.icon;
  return (
    <span className={`cbadge ${onClick ? "clickable" : ""}`} style={{ ["--c" as string]: compColor(c) }} title={ws.info(c)?.about ?? c} onClick={onClick}>
      {icon && <span className="glyph">{icon}</span>}
      {label(c)}
    </span>
  );
}

const only = (v: unknown, key: string): unknown => (v && typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 1 ? (v as Record<string, unknown>)[key] : undefined);

/** What an uncertain number comes to, if that's what this is: `{"dist": …}`. */
export const distOf = (v: unknown): Dist | undefined => only(v, "dist") as Dist | undefined;

/** A number read from draws, if that's what this is: `{"estimate": …}`. */
export const estimateOf = (v: unknown): Estimate | undefined => only(v, "estimate") as Estimate | undefined;

/** An estimate in a few words. It always says it's about that. */
export function estimateText(e: Estimate): string {
  return e.error ? `≈ ${toWithin(e.value, e.error)} ± ${smallFigures(e.error)}` : `≈ ${figures(e.value)}`;
}

export function estimateHow(e: Estimate): string {
  return `Read from ${e.draws.toLocaleString()} draws, not worked out exactly. ${e.error ? `It could be out by about ${smallFigures(e.error)}.` : "It's come through something that doesn't say how far out it could be."}`;
}

/** How likely an uncertain truth is, if that's what this is: `{"chance": …}`. */
export const chanceOf = (v: unknown): Chance | undefined => only(v, "chance") as Chance | undefined;

/** A number for reading. One that isn't there, or isn't a number, says so rather than breaking the page. */
const figures = (n: number | null | undefined) => (typeof n === "number" && Number.isFinite(n) ? valueText(+n.toPrecision(3)) : "–");

/** A small number, such as how far out something could be, to two figures however small it is. */
const smallFigures = (n: number | null | undefined) => (typeof n !== "number" || !Number.isFinite(n) ? "–" : Math.abs(n) >= 1 ? figures(n) : String(+n.toPrecision(2)));

/**
 * A number that could be out by about `error`, written to as many places as that leaves meaningful, so
 * rounding it never moves it further than it could be out anyway.
 */
function toWithin(n: number, error: number): string {
  if (!Number.isFinite(n)) return "–";
  if (!(error > 0)) return figures(n);
  const places = Math.max(0, 1 - Math.floor(Math.log10(error)));
  return n.toLocaleString(undefined, { minimumFractionDigits: 0, maximumFractionDigits: Math.min(places, 12) });
}

/** An uncertain number in a few words: its middle and its spread. What was read from draws says it's about that. */
export function distText(d: Dist): string {
  if (d.undefined >= 1 || !Number.isFinite(d.mean)) return "undefined";
  return `${d.exact ? `${figures(d.mean)}` : `≈ ${toWithin(d.mean, d.error)}`} ± ${smallFigures(d.sd)}`;
}

/** How an uncertain number was got, for a tooltip: its name if it has one, or how many draws and how far out. */
export function distHow(d: Dist): string {
  const range = `Nine times in ten it's between ${figures(d.low)} and ${figures(d.high)}`;
  const gaps = d.undefined > 0 && d.undefined < 1 ? `. Undefined ${beliefText(d.undefined)} of the time` : "";
  if (d.exact) return `${d.law ? `Exactly ${d.law}` : "Exact"}. ${range}${gaps}`;
  return `Read from ${d.draws.toLocaleString()} draws, so the mean could be out by about ${smallFigures(d.error)}. ${range}${gaps}`;
}

/** A chance in a few words. One nothing settled says how many draws it held in, and never says never. */
export function chanceText(c: Chance): string {
  if (c.undefined >= 1) return "undefined";
  if (c.exact) return beliefText(c.chance);
  if (c.hits === 0) return `0 in ${c.draws.toLocaleString()} draws`;
  if (c.hits === c.draws) return `all ${c.draws.toLocaleString()} draws`;
  return `≈ ${beliefText(c.chance)}`;
}

export function chanceHow(c: Chance): string {
  const gaps = c.undefined > 0 ? ` Undefined ${beliefText(c.undefined)} of the time.` : "";
  return c.exact ? `Exactly this likely.${gaps}` : `True in ${c.hits.toLocaleString()} of ${c.draws.toLocaleString()} draws, so it could be out by about ${(c.error * 100).toFixed(1)} points.${gaps}`;
}

/** A histogram, small enough to sit beside a number. */
export function Hist({ d, wide }: { d: Dist; wide?: boolean }) {
  const [w, h] = wide ? [180, 36] : [72, 16];
  const [at, setAt] = useState<number | null>(null);
  const { from, to, bars } = d.hist;
  const span = to - from;
  // Where zero is, if the histogram crosses it, so what's below it can be told from what's above.
  const zero = from < 0 && to > 0 ? (-from / span) * w : null;
  // Only ever one of a few numbers: a bar for each, where it is, as tall as it's likely.
  const points = d.points && d.points.length > 1 && span > 0 ? d.points : null;
  const x = (v: number) => ((v - from) / span) * w;
  const each = w / bars.length;
  const thin = points ? Math.max(1.5, Math.min(6, (w / points.length) * 0.6)) : 0;
  const top = points ? Math.max(...points.map((p) => p[1]), 1e-9) : Math.max(...bars, 1e-9);
  // What's under the pointer: the nearest of the few numbers, or the stretch it's in.
  const near = at === null ? null : points ? points.reduce((best, p, i) => (Math.abs(x(p[0]) - at) < Math.abs(x(points[best][0]) - at) ? i : best), 0) : Math.min(bars.length - 1, Math.max(0, Math.floor(at / each)));
  const about = d.exact ? "" : "about ";
  const tip =
    near === null
      ? null
      : points
        ? `${figures(points[near][0])}: ${about}${beliefText(points[near][1])} of the time`
        : `${figures(from + (span * near) / bars.length)} to ${figures(from + (span * (near + 1)) / bars.length)}: ${about}${beliefText(bars[near])} of the time`;
  return (
    <span className="hist-wrap" onMouseMove={(e) => setAt(e.clientX - e.currentTarget.getBoundingClientRect().left)} onMouseLeave={() => setAt(null)}>
      <svg className={`hist ${d.exact ? "" : "drawn"}`} width={w} height={h} viewBox={`0 0 ${w} ${h}`} role="img" aria-label={`From ${figures(from)} to ${figures(to)}${zero !== null ? ", either side of nothing" : ""}`}>
        {points
          ? points.map(([v, p], i) => <rect key={i} className={near === i ? "on" : undefined} x={Math.min(Math.max(x(v) - thin / 2, 0), w - thin)} y={h - (p / top) * h} width={thin} height={(p / top) * h} />)
          : bars.map((b, i) => <rect key={i} className={near === i ? "on" : undefined} x={i * each} y={h - (b / top) * h} width={Math.max(each - 0.5, 0.5)} height={(b / top) * h} />)}
        {zero !== null && <line x1={zero} x2={zero} y1={0} y2={h} className="hist-zero" />}
        {/* The whole height takes the pointer, so a short bar is as easy to point at as a tall one. */}
        <rect className="hist-hit" x={0} y={0} width={w} height={h} />
      </svg>
      {/* Centred on the pointer, except near an end, where it lines up with that end so it isn't cut off. */}
      {tip && (
        <span className={`hist-tip ${(at ?? 0) < w * 0.3 ? "from-left" : (at ?? 0) > w * 0.7 ? "from-right" : ""}`} style={(at ?? 0) > w * 0.7 ? { right: 0 } : (at ?? 0) < w * 0.3 ? { left: 0 } : { left: at ?? 0 }}>
          {tip}
        </span>
      )}
    </span>
  );
}

/** An uncertain number: its histogram, what it comes to, and how that was got. */
export function DistValue({ d, wide }: { d: Dist; wide?: boolean }) {
  if (d.undefined >= 1 || !Number.isFinite(d.mean) || !d.hist.bars.length) return <span className="mono-value">undefined</span>;
  return (
    <span className={`dist ${wide ? "wide" : ""}`}>
      <Hist d={d} wide={wide} />
      <span className="mono-value" title={distHow(d)}>
        {distText(d)}
      </span>
      {wide && (
        <span className="dist-how muted">
          {figures(d.hist.from)} to {figures(d.hist.to)} · {d.exact ? (d.law ?? "exact") : `from ${d.draws.toLocaleString()} draws`}
          {d.hist.from < 0 && d.hist.to > 0 && d.below0 !== undefined && ` · ${beliefText(d.below0)} of the time under 0`}
        </span>
      )}
    </span>
  );
}

/** An uncertain truth: how likely, and how that was got. */
export function ChanceValue({ c }: { c: Chance }) {
  return (
    <span className="dist" title={chanceHow(c)}>
      <span className="mono-value">true {chanceText(c)}</span>
      {!c.exact && c.hits > 0 && c.hits < c.draws && <span className="dist-how muted">from {c.draws.toLocaleString()} draws</span>}
    </span>
  );
}

/** A value as it's shown: an uncertain one as what it comes to, and anything else in words. */
export function AnyValue({ v, text, wide }: { v: unknown; text: (v: unknown) => string; wide?: boolean }) {
  const d = distOf(v);
  if (d) return <DistValue d={d} wide={wide} />;
  const c = chanceOf(v);
  if (c) return <ChanceValue c={c} />;
  const e = estimateOf(v);
  if (e)
    return (
      <span className="mono-value estimate" title={estimateHow(e)}>
        {estimateText(e)}
      </span>
    );
  return <span className="mono-value">{text(v)}</span>;
}

/** A small grid, wherever something can be looked at as a table. */
export function TableIcon() {
  return (
    <svg className="table-icon" width="13" height="13" viewBox="0 0 13 13" aria-hidden="true">
      <rect x="1" y="1.5" width="11" height="10" rx="1.5" fill="none" stroke="currentColor" />
      <path d="M1 5h11M1 8.25h11M5 5v6.5" fill="none" stroke="currentColor" />
      <path d="M1.5 2h10v3h-10z" fill="currentColor" opacity="0.35" />
    </svg>
  );
}

/** A plain value, in as few words as it can say it. */
export function valueText(v: unknown): string {
  if (v === null || v === undefined) return "yes";
  const d = distOf(v);
  if (d) return distText(d);
  const c = chanceOf(v);
  if (c) return `true ${chanceText(c)}`;
  const e = estimateOf(v);
  if (e) return estimateText(e);
  if (typeof v === "number") return Number.isInteger(v) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  if (typeof v === "string") return v;
  if (typeof v === "boolean") return v ? "yes" : "no";
  return JSON.stringify(v);
}

/** Numbers as a range, and anything else as a short list. Worlds are the point, so never just one of them. */
function casesText(values: unknown[], names?: (s: string) => string): string {
  if (values.length === 1) return displayValue(values[0], names);
  // In words, because a dash between numbers reads as subtracting them.
  // With something uncertain among them, it's where they fall nine times in ten, and says it's about that.
  const ds = values.map((v) => distOf(v));
  if (ds.some((d) => d) && values.every((v, i) => ds[i] || typeof v === "number" || v === null)) {
    const ends = values.flatMap((v, i) => (ds[i] ? [ds[i]!.low, ds[i]!.high] : typeof v === "number" ? [v] : []));
    return `about ${valueText(+Math.min(...ends).toPrecision(3))} to ${valueText(+Math.max(...ends).toPrecision(3))}`;
  }
  const ns = values.filter((v) => typeof v === "number") as number[];
  if (ns.length > 1 && ns.length >= values.length - 1 && values.every((v) => typeof v === "number" || v === null)) {
    const range = `${valueText(Math.min(...ns))} to ${valueText(Math.max(...ns))}`;
    return ns.length === values.length ? range : `${range}, or undefined`;
  }
  const shown = values.map((v) => displayValue(v, names));
  return shown.length > 3 ? `${shown.slice(0, 3).join(" / ")} / …` : shown.join(" / ");
}

/** A node as a value, such as what a choice lands on: `{"ref": handle}`. */
export const refOf = (v: unknown): string | undefined => (v && typeof v === "object" && !Array.isArray(v) && Object.keys(v).length === 1 && typeof (v as { ref?: unknown }).ref === "string" ? (v as { ref: string }).ref : undefined);

function displayValue(v: unknown, names?: (s: string) => string): string {
  if (v === null) return "undefined";
  const ref = refOf(v);
  if (ref) return names ? names(ref) : ref;
  if (typeof v === "string" && names) return names(v);
  return valueText(v);
}

/**
 * How strongly an option of something uncertain is believed now (`chance`: its weight, given whatever rules
 * worlds out and whatever's been pinned or found out), and before any evidence (`weight`). Something that's
 * someone's to choose isn't believed, so it's null.
 */
export function beliefOf(ws: Workspace, decision: string, option: string): { chance: number; weight: number } | null {
  const v = ws.view?.worlds.vars.find((x) => x.id === decision);
  if (!v?.uncertain) return null;
  return v.options.find((o) => o.id === option) ?? null;
}

/** A chance as a percentage, saying when it's almost but not quite certain either way. */
export const beliefText = (p: number) => (typeof p !== "number" || !Number.isFinite(p) ? "–" : p > 0 && p < 0.005 ? "<1%" : p < 1 && p > 0.995 ? ">99%" : `${Math.round(p * 100)}%`);

/** What a belief means, for a tooltip. */
export const BELIEF_MEANS = "How strongly it's believed, given the evidence and anything pinned or found out";

/** What a prior belief means, for a tooltip. */
export const PRIOR_MEANS = "How strongly it was believed before any evidence: its weight, out of all its options'";

/** A cell in one line, for a card or a table. */
export function cellText(cell: Cell, names?: (s: string) => string): string {
  if (cell.error) return "error";
  // Differing only over chance, it's one uncertain thing: what all of that comes to taken together.
  if (cell.mix !== undefined) return valueText(cell.mix);
  if (cell.shown) return shownText(cell.shown, names);
  return valueText(cell.value);
}

export function shownText(s: Shown, names?: (s: string) => string): string {
  switch (s.kind) {
    case "bool":
      return s.when;
    case "cond":
      return casesText(s.cases.map((c) => c.value), names);
    case "named":
      return s.items.length ? s.items.map((i) => names?.(i.node) ?? i.node).join(", ") : "nothing";
  }
}

/** Nearest the pointer first, then on out to the outermost. */
const HELD_COLORS = ["var(--accent)", "var(--c-goal)", "var(--c-task)", "var(--c-question)"];

/** Text with its brackets lit by the pointer: the pair holding it, and every pair around that. */
export function Brackets({ text, onHeld, renderText }: { text: string; /** The innermost pair the pointer is in, if there is one. */ onHeld?: (part: string | null) => void; /** How to show what's between the brackets. */ renderText?: (text: string) => React.ReactNode }) {
  const runs = useMemo(() => bracketRuns(text), [text]);
  const [over, setOver] = useState<number | null>(null);
  if (!runs.some((r) => r.bracket)) return <>{renderText ? renderText(text) : text}</>;
  const held = (over !== null && runs[over]?.held) || [];
  const point = (i: number | null) => {
    setOver(i);
    const open = i === null ? undefined : runs[i]?.held[runs[i].held.length - 1];
    onHeld?.(open === undefined ? null : partText(text, runs, open));
  };
  return (
    <span onMouseLeave={() => point(null)}>
      {runs.map((r, i) => {
        const nth = r.bracket ? held.length - 1 - held.indexOf(r.held[r.held.length - 1]) : held.length;
        const lit = nth < held.length;
        return (
          <span key={i} className={lit ? "bracket-held" : undefined} style={lit ? { color: HELD_COLORS[nth % HELD_COLORS.length] } : undefined} onMouseEnter={() => point(i)}>
            {renderText && !r.bracket ? renderText(r.text) : r.text}
          </span>
        );
      })}
    </span>
  );
}

/** Past this, a formula shows its first alternatives and how many more there are. */
const LONG_FORMULA = 240;

export function Formula({ r }: { r: Reading | string }) {
  const text = typeof r === "string" ? r : r.when;
  const [whole, setWhole] = useState(false);
  const alternatives = text.split(" ∨ ");
  let kept = 1;
  while (kept < alternatives.length && alternatives.slice(0, kept + 1).join(" ∨ ").length <= LONG_FORMULA) kept++;
  const cut = !whole && kept < alternatives.length;
  return (
    <span className={`formula ${text === "always" ? "always" : text === "never" ? "never" : ""}`}>
      <Brackets text={cut ? alternatives.slice(0, kept).join(" ∨ ") : text} />
      {cut && (
        <>
          {" "}
          <button
            className="link small"
            onClick={(e) => {
              e.stopPropagation();
              setWhole(true);
            }}
          >
            ∨ {(alternatives.length - kept).toLocaleString()} more
          </button>
        </>
      )}
    </span>
  );
}

/**
 * How a true-or-false cell reads, the same everywhere. Something like "Needed" is already a question,
 * so it only needs when. A value has to say what it is.
 */
export function truthWords(component: string, r: Reading): { fixed: string } | { lead: string } {
  const isValue = component === "value";
  if (r.always) return { fixed: isValue ? "true" : "always" };
  if (r.never) return { fixed: isValue ? "false" : "never" };
  return { lead: isValue ? "true when" : "when" };
}

export function Truth({ component, r }: { component: string; r: Reading }) {
  const words = truthWords(component, r);
  if ("fixed" in words) return <span className={`formula ${r.always ? "always" : "never"}`}>{words.fixed}</span>;
  return (
    <>
      <span className="muted">{words.lead}</span> <Formula r={r} />
    </>
  );
}

/** More cases than this, where they're all taken together too, are folded away under the summary. */
const MOST_CASES_SHOWN = 6;

/**
 * The cases worth listing. Where they're taken together, they're all matters of chance, so a case that
 * nothing still in play lands on has nothing to say and is left out.
 */
function casesWorthListing(cell: Cell, cases: { when: Reading; value: unknown }[]) {
  return cell.mix === undefined ? cases : cases.filter((c) => c.when.prob > 0 || c.when.always);
}

/** A long list folded under how many there are. */
function Folded({ count, children }: { count: number; children: React.ReactNode }) {
  return (
    <details className="cases-folded">
      <summary className="muted small">each of the {count} ways it could be</summary>
      {children}
    </details>
  );
}

/** What a value means, for the few that aren't plain. */
const MEANING: Record<string, (v: unknown) => string | undefined> = {
  done: (v) => (v === true ? "finished" : v === false ? "still owed" : "nothing to finish"),
};

/** A cell in full: every case it has among the worlds in play. */
export function CellValue({ ws, cell, onSelect, component }: { ws: Workspace; cell: Cell; onSelect: (id: string) => void; component?: string }) {
  const meaning = component ? MEANING[component] : undefined;
  const name = (s: string) => ws.byId.get(s)?.name ?? s;
  const shownValue = (v: unknown) => meaning?.(v) ?? displayValue(v, name);
  if (cell.error) return <span className="cell-error">{cell.error}</span>;
  const s = cell.shown;
  if (!s) {
    if (typeof cell.value === "string" && ws.byId.has(cell.value)) return <NodeChip ws={ws} id={cell.value} onSelect={onSelect} />;
    return <AnyValue v={cell.value} text={valueText} wide />;
  }
  if (s.kind === "bool") return component ? <Truth component={component} r={s} /> : <Formula r={s} />;
  if (s.kind === "named") {
    if (!s.items.length) return <span className="muted">nothing</span>;
    return (
      <span className="named">
        {s.items.map((i) => (
          <span key={i.node} className="named-item">
            <NodeChip ws={ws} id={i.node} onSelect={onSelect} /> <Formula r={i.when} />
          </span>
        ))}
      </span>
    );
  }
  if (s.cases.length === 1) return <AnyValue v={s.cases[0].value} text={shownValue} wide />;
  const cases = casesWorthListing(cell, s.cases);
  const list = cases.map((c, i) => (
    <span key={i} className="case">
      <AnyValue v={c.value} text={shownValue} /> <span className="muted">when</span> <Formula r={c.when} />
    </span>
  ));
  const together = cell.mix !== undefined && (
    <span className="case mix">
      <AnyValue v={cell.mix} text={shownValue} wide /> <span className="muted">taken together</span>
    </span>
  );
  // Many cases taken together read best as the summary, with each a click away.
  if (together && cases.length > MOST_CASES_SHOWN)
    return (
      <span className="cases">
        {together}
        <Folded count={cases.length}>{list}</Folded>
      </span>
    );
  return (
    <span className="cases">
      {list}
      {together}
    </span>
  );
}

/** What a formula comes to, written as following from it. */
export function Resolved({ ws, cell, component }: { ws: Workspace; cell: Cell; component?: string }) {
  const name = (s: string) => ws.byId.get(s)?.name ?? s;
  const meaning = component ? MEANING[component] : undefined;
  const words = (v: unknown) => (v === null ? "undefined" : (meaning?.(v) ?? displayValue(v, name)));
  const read = (v: unknown) => (distOf(v) || chanceOf(v) ? <AnyValue v={v} text={words} wide /> : words(v));
  const row = (key: number | string, value: React.ReactNode, when?: Reading, note?: string) => (
    <div key={key} className="resolved-row">
      <span className="result-value">{value}</span>
      {note && <span className="result-when muted">{note}</span>}
      {when && (
        <span className="result-when">
          <span className="muted">when</span> <Formula r={when} />
        </span>
      )}
    </div>
  );
  if (cell.error) return <div className="cell-error resolved">{cell.error}</div>;
  const s = cell.shown;
  if (!s) return <div className="resolved">{row(0, read(cell.value))}</div>;
  if (s.kind === "bool") return <div className="resolved">{s.always ? row(0, "true") : s.never ? row(0, "false") : row(0, "true", s)}</div>;
  if (s.kind === "named") return <div className="resolved">{row(0, shownText(s, name))}</div>;
  if (s.cases.length === 1) return <div className="resolved">{row(0, read(s.cases[0].value))}</div>;
  const cases = casesWorthListing(cell, s.cases);
  const rows = cases.map((c, i) => row(i, read(c.value), c.when));
  if (cell.mix !== undefined && cases.length > MOST_CASES_SHOWN)
    return (
      <div className="resolved">
        {row("mix", read(cell.mix), undefined, "taken together")}
        <Folded count={cases.length}>
          <div className="resolved">{rows}</div>
        </Folded>
      </div>
    );
  return (
    <div className="resolved">
      {rows}
      {cell.mix !== undefined && row("mix", read(cell.mix), undefined, "taken together")}
    </div>
  );
}

/** A world value as a bool, if that's what it is. */
export function reading(cell: Cell | undefined): Reading | undefined {
  return cell?.shown?.kind === "bool" ? cell.shown : undefined;
}

/** The values a cell can still take. */
export function casesOf(cell: Cell | undefined): unknown[] {
  if (!cell) return [];
  if (cell.shown?.kind === "cond") return cell.shown.cases.map((c) => c.value);
  return cell.value === undefined ? [] : [cell.value];
}

/** The change a tick makes: what the node says for itself goes first, and otherwise it says the opposite of what's worked out. */
export function tickOp(id: string, c: string, cell: Cell): Op {
  if (cell.source === "own") return { op: "set", id, cell: c };
  const cases = casesOf(cell);
  return { op: "set", id, cell: c, value: !(cases.length > 0 && cases.every((v) => v === true)) };
}

/** A true-or-false cell as a box to tick. A node's own formula decides for itself, and differing worlds show as neither. */
export function TickBox({ ws, id, c }: { ws: Workspace; id: string; c: string }) {
  const cell = ws.byId.get(id)?.cells[c];
  if (!cell) return null;
  const cases = casesOf(cell);
  const yes = cases.length > 0 && cases.every((v) => v === true);
  const mixed = !yes && cases.includes(true);
  const own = cell.source === "own";
  const decided = own && cell.formula !== undefined;
  return (
    <input
      type="checkbox"
      aria-label={label(c)}
      checked={yes}
      disabled={decided}
      ref={(el) => {
        if (el) el.indeterminate = mixed;
      }}
      title={decided ? "Its own formula decides this" : own ? "Go back to working this out" : yes ? `Say it isn't ${label(c).toLowerCase()}` : `Mark as ${label(c).toLowerCase()}`}
      onClick={(e) => e.stopPropagation()}
      onChange={() => ws.apply([tickOp(id, c, cell)]).catch((e) => alert(String(e)))}
    />
  );
}

/**
 * Buttons that change something, drawn so they can't be mistaken for one another or for closing a panel,
 * which is always ×. Taking something away is a bin, and going back to what's inherited is an arrow back.
 */
export function RemoveButton({ title, onClick }: { title: string; onClick: () => void }) {
  return (
    <button className="icon small act remove" title={title} aria-label={title} onClick={onClick}>
      <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.7 9.5h6.6L12 4M6.75 6.5v4.5M9.25 6.5v4.5" />
      </svg>
    </button>
  );
}

export function RevertButton({ title, onClick }: { title: string; onClick: () => void }) {
  return (
    <button className="icon small act revert" title={title} aria-label={title} onClick={onClick}>
      <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <path d="M3 3v4h4" />
        <path d="M3.4 7A5 5 0 1 1 4.5 11.5" />
      </svg>
    </button>
  );
}

/** What an edge holds, in a few words: each cell that has something in it. A label says its name. */
export function edgeText(e: Edge): string {
  return Object.entries(e.cells)
    .filter(([c, cell]) => c !== "called" && cell.source !== "default" && cell.value !== undefined)
    .map(([c, cell]) => `${label(c).toLowerCase()} ${typeof cell.value === "string" ? cell.value : JSON.stringify(cell.value)}`)
    .join(" · ");
}

/** Selection keys: a node is its handle, and an edge is "e:" and its handle. */
export const edgeKey = (edge: string) => `e:${edge}`;

export function parseEdgeKey(key: string | null): string | null {
  return key?.startsWith("e:") ? key.slice(2) : null;
}
