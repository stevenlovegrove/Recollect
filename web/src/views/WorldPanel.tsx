import { useEffect, useState, type ReactNode } from "react";
import type { OptionView, VarView, World } from "../api";
import type { ViewProps } from "./MapView";
import type { Workspace } from "../workspace";
import { BELIEF_MEANS, beliefText as percent } from "../bits";
import { DrawingFrame } from "./DrawingFrame";

export const DEFAULT = "default";

/** Which drawing to show is per viewer and per workspace, and it's fine if it can't be remembered. */
export function useRemembered(key: string): [string, (v: string) => void] {
  const [value, setValue] = useState(DEFAULT);
  useEffect(() => {
    try {
      setValue(localStorage.getItem(key) ?? DEFAULT);
    } catch {
      setValue(DEFAULT);
    }
  }, [key]);
  return [
    value,
    (v) => {
      setValue(v);
      try {
        localStorage.setItem(key, v);
      } catch {
        /* it just won't be remembered */
      }
    },
  ];
}

/**
 * What's still possible: every open choice and its options, or a workspace's own drawing of them. Pin
 * one and everything narrows, on every page, with nothing saved. For a puzzle, the worlds left are its
 * solutions. It's what the worlds chip grows into: `summary` is the chip's own line, at its head.
 */
export function WorldPanel({ ws, onSelect, summary }: { ws: Workspace; onSelect: (id: string | null) => void; summary: ReactNode }) {
  const g = ws.view!;
  const w = g.worlds;
  const quantities = w.quantities ?? [];
  const pinned = Object.keys(w.picks).length + quantities.length;
  // What it all means, for whoever asks.
  const [help, setHelp] = useState(false);
  // Stepping through solutions pins every choice, which leaves one, so remember the list it was in.
  const [stepping, setStepping] = useState<{ list: World[]; at: number } | null>(null);
  useEffect(() => {
    if (stepping && JSON.stringify(stepping.list[stepping.at]) !== JSON.stringify(ws.world)) setStepping(null);
  }, [ws.world]); // eslint-disable-line react-hooks/exhaustive-deps
  const step = (by: number) => {
    const list = stepping?.list ?? w.solutions;
    if (!list.length) return;
    const at = stepping ? (stepping.at + by + list.length) % list.length : by > 0 ? 0 : list.length - 1;
    setStepping({ list, at });
    ws.setWorld(list[at]);
  };
  const solutions = `${w.solutions.length}${w.solutions.length === 300 ? "+" : ""}`;

  const drawings = g.nodes.filter((n) => typeof n.cells.drawing?.value === "string");
  const [wanted, setWanted] = useRemembered(`recollect.drawing:${g.workspace.id}`);
  const drawing = drawings.find((n) => n.id === wanted);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => setFailed(null), [drawing?.id]);
  const nameOf = (id: string) => ws.byId.get(id)?.name ?? "something";

  const options = w.vars.flatMap((v) => v.options.map((o) => o.name));
  const shared = w.vars.length > 0 && new Set(options).size === w.vars[0].options.length && w.vars.every((v) => v.options.length === w.vars[0].options.length);
  const atInfo = (v: VarView) => ws.byId.get(v.id)?.cells.at?.value as { row: number; col: number } | undefined;
  const isBoard = w.vars.length > 0 && w.vars.every((v) => atInfo(v));

  return (
    <div className="world-panel">
      {/* The chip it grew from stays at the right end, where it was, with what acts on it before it. */}
      <div className="worlds-head">
        <span className="worlds-tools left">
          {w.constrained && (stepping || w.solutions.length > 1) && (
            <span className="stepper" title={stepping ? `Solution ${stepping.at + 1} of ${stepping.list.length}` : `Step through the ${solutions} solutions`}>
              <button className="icon small" onClick={() => step(-1)} aria-label="Previous solution">
                ◀
              </button>
              <span className="muted small">{stepping ? `${stepping.at + 1}/${stepping.list.length}` : `${solutions} solutions`}</span>
              <button className="icon small" onClick={() => step(1)} aria-label="Next solution">
                ▶
              </button>
            </span>
          )}
          {drawings.length > 0 && (
            <select className="small drawing-pick" value={drawing?.id ?? DEFAULT} onChange={(e) => setWanted(e.target.value)} aria-label="Show" title="Show the choices, or a drawing of them">
              <option value={DEFAULT}>Choices</option>
              {drawings.map((n) => (
                <option key={n.id} value={n.id}>
                  {n.name}
                </option>
              ))}
            </select>
          )}
        </span>
        <span className="worlds-tools right">
          {/* Always there, only hidden with nothing pinned, so pinning the first doesn't move anything. */}
          <button
            className="icon worlds-clear"
            style={pinned ? undefined : { visibility: "hidden" }}
            aria-hidden={!pinned}
            tabIndex={pinned ? undefined : -1}
            aria-label={`Clear ${pinned} pinned`}
            title={`Clear ${pinned} pinned`}
            onClick={() => {
              setStepping(null);
              ws.setWorld({});
            }}
          >
            ⟲
          </button>
        </span>
        {summary}
      </div>
      {quantities.length > 0 && (
        <div className="world-pins">
          {quantities.map((q) => (
            <span key={q.key} className={`world-pin ${q.refused ? "refused" : ""}`} title={q.refused ? `Couldn't be pinned: ${q.refused}` : q.text.includes("..") ? "Known to be in this range: only the draws where it is count" : "Pinned to this value: the same as typing it in"}>
              {nameOf(q.node)}
              {q.cell !== "value" ? ` ${q.cell}` : ""}: <b>{q.text.includes("..") ? `in ${q.text}` : q.text}</b>
              {q.refused && " ⚠"}
              <button className="icon" aria-label="Unpin" onClick={() => ws.pin(q.key, null)}>
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      {drawing && (
        <>
          {failed && (
            <div className="error">
              “{drawing.name}” couldn't be drawn, so these are the choices instead: {failed}{" "}
              <button className="link" onClick={() => onSelect(drawing.id)}>
                Look at its source
              </button>
            </div>
          )}
          <div style={failed ? { display: "none" } : undefined}>
            <DrawingFrame key={drawing.id} ws={ws} source={drawing.cells.drawing.value as string} onFailed={setFailed} />
          </div>
        </>
      )}
      {(!drawing || failed) &&
        (!w.vars.length ? <p className="muted small">Every decision here has been made, so there's only one world.</p> : isBoard ? <Board ws={ws} /> : shared ? <Grid ws={ws} /> : <Lists ws={ws} />)}
      <WorthFindingOut ws={ws} onSelect={onSelect} help={help} />
      {help && (
        <div className="world-help muted small">
          <p>
            Every way the open choices could land is a world. Click an option to pin it, and again to let it go: every page then shows what's true if so, and nothing is
            saved. Solid means pinned, a dot means it's the only one left, and blank means no world lands there.
          </p>
          {w.vars.some((v) => v.uncertain) && <p>For what's uncertain, the percentage is the belief in each: how strongly it's believed given the evidence and whatever's pinned, so it changes as you pin things.</p>}
          {w.constrained && <p>Requirements rule some worlds out, so for a puzzle the worlds left are its solutions, and they can be stepped through.</p>}
        </div>
      )}
      {/* Out of the way, in the corner: what it all means, for whoever asks. */}
      <button className={`icon worlds-help ${help ? "on" : ""}`} aria-pressed={help} aria-label="What this means" title="What this means" onClick={() => setHelp(!help)}>
        ?
      </button>
    </div>
  );
}

function pinner(ws: ViewProps["ws"], v: VarView, o: OptionView) {
  return () => ws.pin(v.id, o.picked ? null : o.id);
}

/** An option as it reads: a row of a table by its key, as a map card does, and anything else by its name. */
function optionName(ws: ViewProps["ws"], o: OptionView): string {
  const row = ws.byId.get(o.id)?.row;
  return row ? String(row.key) : o.name;
}

const onlyLeft = (v: VarView, o: OptionView) => o.possible && !o.picked && v.options.filter((x) => x.possible).length === 1;

/** Choices that all have the same options, as a grid. */
function Grid({ ws }: { ws: ViewProps["ws"] }) {
  const w = ws.view!.worlds;
  const head = w.vars[0].options;
  return (
    <div className="world-grid-wrap">
      <table className="world-grid">
        <thead>
          <tr>
            <th />
            {head.map((o) => (
              <th key={o.id}>{o.name}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {w.vars.map((v) => (
            <tr key={v.id}>
              <th className="world-var" title={v.name}>
                {v.name}
              </th>
              {head.map((h) => {
                const o = v.options.find((x) => x.name === h.name)!;
                return (
                  <td key={h.id} className={`world-cell ${o.picked ? "picked" : ""} ${onlyLeft(v, o) ? "forced" : ""} ${o.possible ? "possible" : "gone"}`}>
                    <button onClick={pinner(ws, v, o)} aria-label={`${v.name}: ${o.name}`} title={`${v.name}: ${o.name}`}>
                      {o.picked ? "●" : onlyLeft(v, o) ? "•" : ""}
                    </button>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Choices placed on a board, each showing what's still possible where it is. */
function Board({ ws }: { ws: ViewProps["ws"] }) {
  const w = ws.view!.worlds;
  const at = (v: VarView) => ws.byId.get(v.id)!.cells.at.value as { row: number; col: number };
  const rows = Math.max(...w.vars.map((v) => at(v).row)) + 1;
  const cols = Math.max(...w.vars.map((v) => at(v).col)) + 1;
  return (
    <div className="board" style={{ gridTemplateColumns: `repeat(${cols}, 88px)`, gridTemplateRows: `repeat(${rows}, 88px)` }}>
      {w.vars.map((v) => {
        const { row, col } = at(v);
        const possible = v.options.filter((o) => o.possible);
        const picked = v.options.find((o) => o.picked);
        return (
          <div key={v.id} className={`board-cell ${picked ? "picked" : ""} ${!possible.length ? "none" : ""}`} style={{ gridRow: row + 1, gridColumn: col + 1 }} title={v.name}>
            {picked ? (
              <button className="board-big" onClick={() => ws.pin(v.id, null)} title="Unpin">
                {picked.name}
              </button>
            ) : possible.length === 1 ? (
              <button className="board-big forced" onClick={() => ws.pin(v.id, possible[0].id)}>
                {possible[0].name}
              </button>
            ) : (
              <span className="board-small">
                {v.options.map((o) => (
                  <button key={o.id} disabled={!o.possible} onClick={() => ws.pin(v.id, o.id)}>
                    {o.possible ? o.name : ""}
                  </button>
                ))}
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** Choices with options of their own. */
function Lists({ ws }: { ws: ViewProps["ws"] }) {
  const w = ws.view!.worlds;
  return (
    <div className="world-lists">
      {w.vars.map((v) => (
        <div key={v.id} className="world-list">
          <div className="world-var">{v.name}</div>
          <div className="chips">
            {v.options.map((o) => (
              <button key={o.id} className={`world-opt ${o.picked ? "picked" : ""} ${o.possible ? "" : "gone"} ${o.gap ? "gap" : ""}`} onClick={pinner(ws, v, o)} title={o.name}>
                {optionName(ws, o)}
                {v.uncertain && v.options.length > 1 && (o.chance > 0 || !o.gap) && (
                  <span className="muted small" title={`Belief: ${BELIEF_MEANS.toLowerCase()}. Prior belief, before any evidence: ${percent(o.weight)}.`}>
                    {" "}
                    {percent(o.chance)}
                  </span>
                )}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}


/**
 * For something uncertain, what's most worth finding out next: each other open question, by how sure of
 * the answer you'd be on average once you knew it. Pinning one is finding it out.
 */
function WorthFindingOut({ ws, onSelect, help }: { ws: ViewProps["ws"]; onSelect: (id: string) => void; help: boolean }) {
  const w = ws.view!.worlds;
  const open = w.vars.filter((v) => v.sure !== undefined);
  // What the case is about: whatever has the most answers, such as who it was, which stays put as things
  // are found out.
  const usual = [...open].sort((a, b) => b.options.length - a.options.length || (b.clues?.length ?? 0) - (a.clues?.length ?? 0))[0];
  const [wanted, setWanted] = useState<string | null>(null);
  const target = open.find((v) => v.id === wanted) ?? usual;
  if (!target || !w.vars.some((v) => v.clues?.length)) return null;
  const likeliest = [...target.options].sort((a, b) => b.chance - a.chance)[0];
  const nameOf = (id: string) => w.vars.find((v) => v.id === id)?.name ?? id;
  return (
    <section className="worth-finding-out">
      <h4>
        Worth finding out, for{" "}
        <select className="small" value={target.id} onChange={(e) => setWanted(e.target.value)} aria-label="What to work out">
          {open.map((v) => (
            <option key={v.id} value={v.id}>
              {v.name}
            </option>
          ))}
        </select>
      </h4>
      <p className="muted small">
        Believed most now: {optionName(ws, likeliest)}, {percent(likeliest.chance)}.{help && target.clues?.length ? " Each of these, once known, would leave you this sure on average." : ""}
      </p>
      {!target.clues?.length && <p className="muted small">Nothing else that's open would change which is believed most, on its own.</p>}
      <ol>
        {(target.clues ?? []).map((c) => (
          <li key={c.var}>
            <button className="link" onClick={() => onSelect(c.var)}>
              {nameOf(c.var)}
            </button>{" "}
            <span className="muted small">
              {percent(target.sure!)} → {percent(c.sure)}
            </span>
          </li>
        ))}
      </ol>
    </section>
  );
}
