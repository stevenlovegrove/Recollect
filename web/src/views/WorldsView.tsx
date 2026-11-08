import { useEffect, useState } from "react";
import type { Cell, GraphNode, OptionView, VarView, World } from "../api";
import type { ViewProps } from "./MapView";
import { CellValue } from "../bits";
import { label } from "../theme";
import { DrawingFrame } from "./DrawingFrame";

const DEFAULT = "default";

/** Which drawing to show is per viewer and per workspace, and it's fine if it can't be remembered. */
function useRemembered(key: string): [string, (v: string) => void] {
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
 * What's still possible. Every open choice and its options: pin one and everything narrows, on this
 * tab and everywhere else, with nothing saved. For a puzzle, the worlds left are its solutions.
 */
export function WorldsView({ ws, onSelect }: ViewProps) {
  const g = ws.view!;
  const w = g.worlds;
  const pinned = Object.keys(w.picks).length;
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

  const drawings = g.nodes.filter((n) => typeof n.cells.drawing?.value === "string");
  const [wanted, setWanted] = useRemembered(`recollect.drawing:${g.workspace.dir}`);
  const drawing = drawings.find((n) => n.id === wanted);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => setFailed(null), [drawing?.id]);
  const [remembered, setShowing] = useRemembered(`recollect.worlds-showing:${g.workspace.dir}`);
  const showing = remembered === DEFAULT ? "left" : remembered;

  if (!w.vars.length && !drawing && !drawings.length) {
    return (
      <div className="pad empty">
        <h3>Nothing is open</h3>
        <p className="muted">Every decision here has been made, so there's only one world.</p>
      </div>
    );
  }

  const options = w.vars.flatMap((v) => v.options.map((o) => o.name));
  const shared = w.vars.length > 0 && new Set(options).size === w.vars[0].options.length && w.vars.every((v) => v.options.length === w.vars[0].options.length);
  const atInfo = (v: VarView) => ws.byId.get(v.id)?.cells.at?.value as { row: number; col: number } | undefined;
  const isBoard = w.vars.length > 0 && w.vars.every((v) => atInfo(v));
  const summaries = g.components.filter((c) => c.summary);
  // Differs between the worlds left, or in any world at all, whatever's pinned or ruled out, as a
  // puzzle's constraints are.
  const differsLeft = (c: Cell) => !!c.vars?.length;
  const differsAny = (c: Cell) => !!c.varies;
  const differs = (c: Cell) => (showing === "all" ? true : showing === "any" ? differsAny(c) : differsLeft(c));
  // What a node says that's worth a column: what its systems summarise it by, and any of the workspace's
  // own cells that are worked out, like a car's cost a year.
  const shownOf = (n: GraphNode) => [...summaries.map((c) => c.name).filter((k) => n.cells[k]), ...Object.keys(n.cells).filter((k) => n.cells[k].workspace && n.cells[k].source === "rule")];
  // A choice and its options say what they are above.
  const above = new Set(w.vars.flatMap((v) => [v.id, ...v.options.map((o) => o.id)]));
  const values = g.nodes.filter((n) => !above.has(n.id) && shownOf(n).some((k) => differs(n.cells[k])));

  return (
    <div className="scroll pad worlds">
      <div className="worlds-head">
        <div className={`world-count ${w.count === "0" ? "none" : ""}`}>
          <b>{w.count === "0" ? "No worlds" : Number(w.count).toLocaleString()}</b>{" "}
          <span className="muted">{w.count === "0" ? "are consistent with these" : w.count === "1" ? "world left" : "worlds left"}</span>
        </div>
        {w.constrained && (stepping || w.solutions.length > 1) && (
          <span className="stepper">
            <button onClick={() => step(-1)} title="Previous solution">
              ◀
            </button>
            <span className="muted small">{stepping ? `Solution ${stepping.at + 1} of ${stepping.list.length}` : `Step through ${w.solutions.length}${w.solutions.length === 300 ? "+" : ""}`}</span>
            <button onClick={() => step(1)} title="Next solution">
              ▶
            </button>
          </span>
        )}
        {pinned > 0 && (
          <button
            onClick={() => {
              setStepping(null);
              ws.setWorld({});
            }}
          >
            Clear {pinned} pinned
          </button>
        )}
        {drawings.length > 0 && (
          <label className="drawing-pick small">
            <span className="muted">Show</span>{" "}
            <select value={drawing?.id ?? DEFAULT} onChange={(e) => setWanted(e.target.value)}>
              <option value={DEFAULT}>Default</option>
              {drawings.map((n) => (
                <option key={n.id} value={n.id}>
                  {n.name}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {drawing && (
        <>
          {failed && (
            <div className="error">
              “{drawing.name}” couldn't be drawn, so this is the default view: {failed}{" "}
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
      {(!drawing || failed) && (
        <>
          <p className="muted small">Click to pin a choice. Solid means pinned, a dot means it's the only one left, and blank means no world lands there.</p>
          {!w.vars.length ? <p className="muted">Every decision here has been made, so there's only one world.</p> : isBoard ? <Board ws={ws} /> : shared ? <Grid ws={ws} /> : <Lists ws={ws} />}
        </>
      )}
      <WorthFindingOut ws={ws} onSelect={onSelect} />

      {g.nodes.some((n) => !above.has(n.id) && shownOf(n).length) && (
        <section className="world-values">
          <h4>
            What that makes things{" "}
            <select className="small" value={showing} onChange={(e) => setShowing(e.target.value)}>
              <option value="left">that differ between the worlds left</option>
              <option value="any">that differ between any worlds</option>
              <option value="all">all of them</option>
            </select>
          </h4>
          {!values.length && <p className="muted small">Nothing does.</p>}
          <table className="sheet">
            <tbody>
              {values.map((n) => (
                <tr key={n.id} onClick={() => onSelect(n.id)}>
                  <td className="sheet-name">{n.name}</td>
                  {shownOf(n).map((k) => (
                    <td key={k}>
                      <span className="muted small">{label(k)}</span> <CellValue ws={ws} cell={n.cells[k]} onSelect={onSelect} component={k} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
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
                {v.options.length > 1 && (o.chance > 0 || !o.gap) && (
                  <span className="muted small" title={`${percent(o.weight)} before the evidence and anything pinned`}>
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

const percent = (p: number) => (p > 0 && p < 0.005 ? "<1%" : p < 1 && p > 0.995 ? ">99%" : `${Math.round(p * 100)}%`);

/**
 * For something uncertain, what's most worth finding out next: each other open question, by how sure of
 * the answer you'd be on average once you knew it. Pinning one is finding it out.
 */
function WorthFindingOut({ ws, onSelect }: { ws: ViewProps["ws"]; onSelect: (id: string) => void }) {
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
        Likeliest now: {optionName(ws, likeliest)}, {percent(likeliest.chance)}.{target.clues?.length ? " Each of these, once known, would leave you this sure on average." : ""}
      </p>
      {!target.clues?.length && <p className="muted small">Nothing else that's open would change which is likeliest, on its own.</p>}
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
