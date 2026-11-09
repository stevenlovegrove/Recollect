import { useEffect, useState } from "react";
import { api, type Cell, type Matrix, type MatrixOption, type MatrixRow, type VarView, type View } from "../api";
import type { ViewProps } from "./MapView";
import type { Workspace } from "../workspace";
import { BELIEF_MEANS, Formula, NodeChip, PRIOR_MEANS, beliefOf, beliefText, casesOf, valueText } from "../bits";
import { SweepChart, type Line } from "./SweepChart";
import { nodeColor, pct } from "../theme";
import { askWhy } from "../Ask";

type Focus = { id: string; n: number } | null;

/**
 * A board of decisions. Each shows what its options need, and looking at an option narrows the whole
 * app to it, with nothing saved. Deciding is then one fact on the decision: nothing beneath it is
 * ever touched, so reopening it brings everything back.
 */
export function DecisionsView({ ws, selected, onSelect, focus }: ViewProps & { focus?: Focus }) {
  const open = ws.plan.decisions.filter((m) => !m.chosen);
  const decided = ws.plan.decisions.filter((m) => m.chosen);
  useEffect(() => {
    if (focus) setTimeout(() => document.getElementById(`decision-${focus.id}`)?.scrollIntoView({ behavior: "smooth", block: "start" }), 50);
  }, [focus?.n]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!ws.plan.decisions.length) {
    return (
      <div className="pad empty">
        <h3>No decisions yet</h3>
        <p className="muted">Give a node a ⑂ Decision, then link its options to it. Work the options share shows up here as safe to start before you decide.</p>
      </div>
    );
  }
  return (
    <div className="scroll pad">
      {open.map((m) => (
        <DecisionCard key={m.id} m={m} ws={ws} selected={selected} onSelect={onSelect} focused={focus?.id === m.id} />
      ))}
      {decided.length > 0 && <h3 className="section-head">Decided</h3>}
      {decided.map((m) => (
        <DecisionCard key={m.id} m={m} ws={ws} selected={selected} onSelect={onSelect} focused={focus?.id === m.id} />
      ))}
    </div>
  );
}

function DecisionCard({ m, ws, selected, onSelect, focused }: { m: Matrix; ws: Workspace; selected: string | null; onSelect: (id: string | null) => void; focused: boolean }) {
  const [err, setErr] = useState<string | null>(null);
  const [committing, setCommitting] = useState<{ kind: "choose" | "rule_out"; option: MatrixOption } | null>(null);
  const looking = ws.world[m.id];
  const run = (ops: Parameters<Workspace["apply"]>[0], why: string) => {
    setErr(null);
    return ws.apply(ops, why).then(
      () => true,
      (e) => {
        setErr(e instanceof Error ? e.message : String(e));
        return false;
      },
    );
  };
  const safe = m.rows.filter((r) => r.is_work && r.need.always && !r.finished).length;
  const specific = m.rows.filter((r) => r.is_work && !r.need.always && !r.need.never && !r.finished).length;
  const live = m.options.filter((o) => !o.ruled_out && !o.dropped);
  const uncertain = ws.view?.worlds.vars.find((v) => v.id === m.id)?.uncertain;

  return (
    <article id={`decision-${m.id}`} className={`choice-card ${focused ? "focused" : ""} ${m.chosen ? "decided" : ""}`}>
      <header>
        <button className="choice-title" onClick={() => onSelect(m.id)}>
          ⑂ {m.name}
        </button>
        <span className={`pressure p-${uncertain && !m.chosen ? "idle" : m.pressure.state}`}>{standing(m, !!uncertain)}</span>
      </header>

      <div className="opt-list">
        {m.options.map((o) => {
          const out = o.ruled_out || o.dropped;
          return (
            <div key={o.id} className={`opt-line ${o.chosen ? "chosen" : ""} ${out ? "out" : ""} ${looking === o.id ? "looking" : ""}`}>
              <span className="opt-mark">{o.chosen ? "✓" : out ? "✕" : o.gap ? "…" : "○"}</span>
              <button className="opt-name" onClick={() => onSelect(o.id)} title={o.gap ? "A gap: an option nobody has named" : undefined}>
                {o.name}
              </button>
              {!m.chosen && !out && (() => {
                const b = beliefOf(ws, m.id, o.id);
                return b !== null && (b.chance > 0 || !o.gap) ? (
                  <span className="muted small">
                    <span title={BELIEF_MEANS}>belief {beliefText(b.chance)}</span>
                    {beliefText(b.chance) !== beliefText(b.weight) && <span title={PRIOR_MEANS}>, prior belief {beliefText(b.weight)}</span>}
                  </span>
                ) : null;
              })()}
              {o.ruled_out && <span className="muted small">ruled out</span>}
              {o.dropped && <span className="muted small">dropped</span>}
              <span className="opt-actions">
                {!m.chosen && !out && (
                  <button className={`link ${looking === o.id ? "on" : ""}`} title="Narrow everything to this option, with nothing saved" onClick={() => ws.pin(m.id, looking === o.id ? null : o.id)}>
                    {looking === o.id ? "stop looking" : "look"}
                  </button>
                )}
                {!m.chosen && !out && !o.gap && (
                  <button className="link" title={uncertain ? "Record that this is how it was" : undefined} onClick={() => setCommitting({ kind: "choose", option: o })}>
                    {uncertain ? "found out…" : "choose…"}
                  </button>
                )}
                {!m.chosen && !out && !o.gap && live.length > 1 && (
                  <button className="link" onClick={() => setCommitting({ kind: "rule_out", option: o })}>
                    rule out…
                  </button>
                )}
                {!m.chosen && o.gap && (
                  <button className="link" title="Say these are all the options there are" onClick={async () => {
                    const why = await askWhy(`Close “${o.name}”, saying these are all the options there are?`, "Close the gap");
                    if (why !== null) run([{ op: "delete_node", id: o.id }], why);
                  }}>
                    close the gap…
                  </button>
                )}
                {o.ruled_out && (
                  <button className="link" onClick={() => run([{ op: "set", from: o.id, to: m.id, cell: "ruled_out" }], "brought back")}>
                    bring back
                  </button>
                )}
                {o.chosen && (
                  <button className="link" onClick={() => run([{ op: "set", id: m.id, cell: "chosen" }], "reopened")}>
                    reopen
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>

      {!m.chosen && uncertain && <WorthKnowing m={m} ws={ws} onSelect={onSelect} />}
      {!m.chosen && <CompareTable m={m} ws={ws} onSelect={onSelect} />}
      {committing && <CommitPanel key={committing.option.id + committing.kind} m={m} ws={ws} commit={committing} onSelect={onSelect} onCancel={() => setCommitting(null)} run={run} />}
      {err && <div className="error">{err}</div>}
      {(safe > 0 || specific > 0) && !m.chosen && (
        <p className="muted small">
          {safe} task{safe === 1 ? "" : "s"} needed whichever way this goes, {specific} that depend on it.
        </p>
      )}
      {m.rows.length > 0 && <MatrixTable m={m} ws={ws} selected={selected} onSelect={onSelect} />}
      {m.rows.length === 0 && <p className="muted small">Nothing has been broken down beneath this yet.</p>}
    </article>
  );
}

/** Where a decision stands, in a few words. Something uncertain isn't waiting on anyone to decide it. */
function standing(m: Matrix, uncertain: boolean): string {
  const n = m.pressure.work.length;
  if (uncertain && !m.chosen) return "Not yours to choose: it happened one way. Belief is how strongly each is believed given the evidence, and prior belief how strongly before it";
  switch (m.pressure.state) {
    case "free":
      return `${n} safe step${n === 1 ? "" : "s"} ready: no need to decide yet`;
    case "blocking":
      return `Holding up ${n} task${n === 1 ? "" : "s"}`;
    case "idle":
      return "Nothing to start beneath it yet";
    default:
      return "Decided";
  }
}

/** Before deciding: what it would change, worked out from the same exact values. Nothing beneath a decision is touched by it. */
function CommitPanel({ m, ws, commit, onSelect, onCancel, run }: { m: Matrix; ws: Workspace; commit: { kind: "choose" | "rule_out"; option: MatrixOption }; onSelect: (id: string | null) => void; onCancel: () => void; run: (ops: Parameters<Workspace["apply"]>[0], why: string) => Promise<boolean> }) {
  const [why, setWhy] = useState("");
  const o = commit.option;
  const choosing = commit.kind === "choose";
  const uncertain = ws.view?.worlds.vars.find((v) => v.id === m.id)?.uncertain;
  const open = m.rows.filter((r) => r.is_work && !r.finished);
  const safe = choosing ? open.filter((r) => !r.need.always && r.per_option[o.id] >= 0.999) : [];
  const notNeeded = choosing ? m.rows.filter((r) => !r.need.never && !r.finished && (r.per_option[o.id] ?? 0) <= 0) : m.rows.filter((r) => !r.need.never && !r.finished && m.options.every((x) => x.id === o.id || x.ruled_out || x.dropped || (r.per_option[x.id] ?? 0) <= 0));
  const confirm = async () => {
    const reason = why.trim();
    const ops: Parameters<Workspace["apply"]>[0] = choosing ? [{ op: "set", id: m.id, cell: "chosen", value: { ref: o.id } }] : [{ op: "set", from: o.id, to: m.id, cell: "ruled_out", value: reason || true }];
    if (await run(ops, reason)) {
      if (ws.world[m.id]) ws.pin(m.id, null);
      onCancel();
    }
  };
  return (
    <div className="commit-panel">
      <h4>{choosing ? (uncertain ? `Found out: “${o.name}”` : `Choose “${o.name}”`) : `Rule out “${o.name}”`}</h4>
      <ul className="consequences">
        {safe.length > 0 && (
          <li>
            <b>{safe.length}</b> task{safe.length === 1 ? "" : "s"} would be needed for certain:{" "}
            {safe.slice(0, 6).map((r) => (
              <NodeChip key={r.id} ws={ws} id={r.id} onSelect={onSelect} />
            ))}
          </li>
        )}
        {notNeeded.length > 0 && (
          <li>
            <b>{notNeeded.length}</b> thing{notNeeded.length === 1 ? "" : "s"} would no longer be needed:{" "}
            {notNeeded.slice(0, 6).map((r) => (
              <NodeChip key={r.id} ws={ws} id={r.id} onSelect={onSelect} />
            ))}
          </li>
        )}
        {safe.length === 0 && notNeeded.length === 0 && <li className="muted">Nothing beneath it would change.</li>}
        <li className="muted">Nothing is written onto that work, so taking this back brings it all back.</li>
      </ul>
      <textarea autoFocus rows={2} placeholder={uncertain && choosing ? "How do you know? This is recorded in the log." : "Why? This is recorded in the log."} value={why} onChange={(e) => setWhy(e.target.value)} />
      <div className="row end">
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" onClick={confirm}>
          {choosing ? (uncertain ? "Record it" : "Decide") : "Rule out"}
        </button>
      </div>
    </div>
  );
}

function MatrixTable({ m, ws, selected, onSelect }: { m: Matrix; ws: Workspace; selected: string | null; onSelect: (id: string | null) => void }) {
  return (
    <table className="matrix">
      <thead>
        <tr>
          <th className="t">Work beneath</th>
          {m.options.map((o) => (
            <th key={o.id} className={o.ruled_out || o.dropped ? "off" : ws.world[m.id] && ws.world[m.id] !== o.id ? "off" : ""} title={o.name}>
              {o.name.length > 10 ? `${o.name.slice(0, 9)}…` : o.name}
            </th>
          ))}
          <th>Needed when</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {m.rows.map((r) => (
          <tr key={r.id} className={`${r.id === selected ? "selected" : ""} ${r.finished ? "done" : ""} ${r.need.never ? "pruned" : ""} ${ws.recent.has(r.id) ? "recent" : ""}`} onClick={() => onSelect(r.id)}>
            <td className="t" style={{ paddingLeft: 6 + (r.depth - 1) * 14 }}>
              <span className="chip-dot" style={{ background: ws.byId.get(r.id) ? nodeColor(ws.byId.get(r.id)!) : undefined }} /> {r.name}
            </td>
            {m.options.map((o) => (
              <td key={o.id} className={o.ruled_out || o.dropped ? "off" : ""}>
                <NeedDot p={r.per_option[o.id] ?? 0} />
              </td>
            ))}
            <td>
              <Formula r={r.need} />
            </td>
            <td>
              <RowTag r={r} ws={ws} onSelect={onSelect} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

type Column = { node: string; cell: string; title: string; of: "option" | "elsewhere" };

/**
 * The options side by side: what each says about itself, and whatever else comes out differently
 * depending on which is taken. Each is worked out by asking as if that option were taken, which is the
 * same as really deciding, with every other choice still open.
 */
function CompareTable({ m, ws, onSelect }: { m: Matrix; ws: Workspace; onSelect: (id: string | null) => void }) {
  const live = m.options.filter((o) => !o.ruled_out && !o.dropped && !o.gap);
  const [views, setViews] = useState<Record<string, View> | null>(null);
  const [charting, setCharting] = useState(false);
  const [chartCol, setChartCol] = useState<string | null>(null);
  const key =`${ws.view?.last_seq}|${JSON.stringify(ws.world)}|${live.map((o) => o.id).join()}`;
  useEffect(() => {
    let alive = true;
    Promise.all(live.map((o) => api.view({ ...ws.world, [m.id]: o.id }).then((v) => [o.id, v] as const)))
      .then((rs) => alive && setViews(Object.fromEntries(rs)))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!views || live.length < 2) return null;

  const cellIn = (o: string, node: string, cell: string) => views[o]?.nodes.find((n) => n.id === node)?.cells[cell];
  const options = new Set(m.options.map((o) => o.id));
  const columns: Column[] = [];
  // What the options say about themselves: their own cells, by the names formulas use.
  const own = new Set(live.flatMap((o) => Object.entries(ws.byId.get(o.id)?.cells ?? {}).filter(([c]) => !ws.cellInfo(c)).map(([c]) => c)));
  for (const c of [...own].sort()) columns.push({ node: "", cell: c, title: c, of: "option" });
  // Anything else whose value isn't the same whichever is taken.
  for (const n of ws.view!.nodes) {
    if (options.has(n.id) || n.id === m.id) continue;
    for (const c of Object.keys(n.cells)) {
      if (c !== "value" && ws.cellInfo(c)) continue;
      const seen = new Set(live.map((o) => JSON.stringify(cellIn(o.id, n.id, c)?.shown ?? cellIn(o.id, n.id, c)?.value ?? null)));
      if (seen.size > 1) columns.push({ node: n.id, cell: c, title: c === "value" ? n.name : `${n.name} · ${c}`, of: "elsewhere" });
    }
  }
  if (!columns.length) return null;

  // A chart reads one column for each option, pinned to it, as some other number changes.
  const colKey = (c: Column) => c.node + "|" + c.cell;
  const at = (o: string, col: Column) => cellIn(o, col.of === "option" ? o : col.node, col.cell);
  const numeric = columns.filter((col) => live.some((o) => casesOf(at(o.id, col)).some((v) => typeof v === "number")));
  // Something worked out is more worth charting than a price typed in, and something that doesn't wait on
  // another choice can be drawn as a line.
  const choice = (id: string) => ws.view!.worlds.vars.some((v) => v.id === id && !v.uncertain);
  // What to vary is a number of its own, or something uncertain as a whole, not one of its options.
  const optionOfAny = new Set(ws.view!.edges.filter((l) => l.components.includes("alternative")).map((l) => l.from));
  const drawable = numeric.filter((col) => live.every((o) => !(at(o.id, col)?.vars ?? []).some(choice)));
  const worked = drawable.find((col) => live.some((o) => at(o.id, col)?.formula)) ?? drawable[0] ?? numeric[0];
  const charted = numeric.find((c) => colKey(c) === chartCol) ?? worked;
  const inputs = ws
    .view!.nodes.filter((n) => !optionOfAny.has(n.id) && n.id !== m.id && !choice(n.id) && casesOf(n.cells.value).some((v) => typeof v === "number"))
    .map((n) => ({ node: n.id, name: n.name, uncertain: !!ws.view!.worlds.vars.find((v) => v.id === n.id)?.uncertain }))
    .sort((a, b) => Number(b.uncertain) - Number(a.uncertain));
  const lines: Line[] = charted ? live.map((o) => ({ id: o.id, name: o.name, read: { node: charted.of === "option" ? o.id : charted.node, cell: charted.cell }, world: { ...ws.world, [m.id]: o.id } })) : [];

  return (
    <div className="compare-wrap">
    <table className="matrix compare">
      <thead>
        <tr>
          <th className="t">If it's</th>
          {columns.map((col) => (
            <th key={col.node + col.cell} className={col.of === "elsewhere" ? "elsewhere" : ""} title={col.of === "elsewhere" ? `${col.title}, if this option is taken` : `Each option's ${col.cell}`}>
              {col.title}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {live.map((o) => (
          <tr key={o.id} className={ws.world[m.id] === o.id ? "selected" : ""} onClick={() => onSelect(o.id)}>
            <td className="t">{o.name}</td>
            {columns.map((col) => (
              <td key={col.node + col.cell}>
                <Summary cell={at(o.id, col)} vars={views[o.id].worlds.vars} />
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
    {charted && inputs.length > 0 && (
      <div className="sweep-open small">
        <button className="link" onClick={() => setCharting(!charting)}>
          {charting ? "hide the chart" : "chart how these change…"}
        </button>
        {charting && numeric.length > 1 && (
          <select value={colKey(charted)} onChange={(e) => setChartCol(e.target.value)} aria-label="What to chart">
            {numeric.map((c) => (
              <option key={colKey(c)} value={colKey(c)}>
                {c.title}
              </option>
            ))}
          </select>
        )}
      </div>
    )}
    {charting && charted && inputs.length > 0 && <SweepChart ws={ws} lines={lines} inputs={inputs} title={charted.title} />}
    </div>
  );
}

/**
 * A cell in a few characters: true or false, or a number and its range over what's still open. What's
 * expected only averages over what's uncertain: over a choice still to be made, it means nothing, so
 * that's a range, saying which choices it's waiting on.
 */
function Summary({ cell, vars }: { cell: Cell | undefined; vars: VarView[] }) {
  if (!cell) return <span className="muted">–</span>;
  if (cell.error) return <span className="cell-error">error</span>;
  const s = cell.shown;
  const choices = (cell.vars ?? []).map((id) => vars.find((v) => v.id === id)).filter((v) => v && !v.uncertain) as VarView[];
  const waiting = choices.length ? `depends on ${choices.map((v) => v.name).join(" and ")}` : null;
  if (s?.kind === "bool") {
    if (s.always) return <span className="yes">✓</span>;
    if (s.never) return <span className="no">✕</span>;
    return (
      <span title={s.when}>
        {waiting ? <span className="muted small">{waiting}</span> : <>✓ <span className="muted small">{pct(s.prob)}</span></>}
      </span>
    );
  }
  if (s?.kind === "cond") {
    const nums = s.cases.filter((c) => typeof c.value === "number");
    const total = nums.reduce((t, c) => t + c.when.prob, 0);
    if (nums.length === s.cases.length && total > 0) {
      const mean = nums.reduce((t, c) => t + (c.value as number) * c.when.prob, 0) / total;
      const values = nums.map((c) => c.value as number);
      const [lo, hi] = [Math.min(...values), Math.max(...values)];
      const cases = s.cases.map((c) => `${valueText(c.value)} when ${c.when.when}`).join("\n");
      if (lo !== hi && waiting) {
        return (
          <span title={cases}>
            {valueText(lo)} to {valueText(hi)}
            <span className="range muted small">{waiting}</span>
          </span>
        );
      }
      return (
        <span title={cases}>
          {valueText(lo === hi ? lo : mean)}
          {lo !== hi && (
            <span className="range muted small">
              {valueText(lo)} to {valueText(hi)}
            </span>
          )}
        </span>
      );
    }
    if (s.cases.length === 1) return <span>{valueText(s.cases[0].value)}</span>;
  }
  return <span>{valueText(cell.value)}</span>;
}

function NeedDot({ p }: { p: number }) {
  if (p <= 0) return null;
  if (p >= 0.999) return <span className="need full" title="needed" />;
  return <span className="need part" title={`needed in ${pct(p)} of this option's worlds`} style={{ ["--p" as string]: `${p * 360}deg` }} />;
}

function RowTag({ r, ws, onSelect }: { r: MatrixRow; ws: Workspace; onSelect: (id: string) => void }) {
  if (r.finished) return <span className="tagpill done">done</span>;
  if (r.need.never) return <span className="tagpill">not needed</span>;
  if (r.need.always || r.need.prob >= 0.999) {
    if (!r.is_work) return <span className="tagpill safe-soft">safe</span>;
    if (r.blockers.length)
      return (
        <span className="tagpill safe-soft" onClick={(e) => e.stopPropagation()} title="Safe, but waiting on something">
          safe, waits for <NodeChip ws={ws} id={r.blockers[0].id} onSelect={onSelect} />
        </span>
      );
    return <span className="tagpill safe">safe now</span>;
  }
  return <span className={`tagpill ${r.need.prob >= 0.5 ? "hedge" : ""}`}>{r.need.prob >= 0.5 ? "hedge" : "commits"}</span>;
}

/** For something uncertain: which other open question would most help to know, as the Worlds tab says. */
function WorthKnowing({ m, ws, onSelect }: { m: Matrix; ws: Workspace; onSelect: (id: string | null) => void }) {
  const v = ws.view?.worlds.vars.find((x) => x.id === m.id);
  if (!v || v.sure === undefined) return null;
  const nameOf = (id: string) => ws.view!.worlds.vars.find((x) => x.id === id)?.name ?? id;
  const best = v.clues?.[0];
  return (
    <p className="muted small">
      {best ? (
        <>
          Most worth finding out:{" "}
          <button className="link" onClick={() => onSelect(best.var)}>
            {nameOf(best.var)}
          </button>
          . Knowing it would leave you {beliefText(best.sure)} sure of this on average, from {beliefText(v.sure)} now.
        </>
      ) : (
        <>Nothing else that's open would change which is believed most, on its own.</>
      )}
    </p>
  );
}
