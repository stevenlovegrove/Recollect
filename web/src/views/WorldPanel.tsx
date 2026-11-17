import { useEffect, useState, type ReactNode } from "react";
import type { OptionView, VarView, World } from "../api";
import type { ViewProps } from "./MapView";
import type { Workspace } from "../workspace";
import { BELIEF_MEANS, beliefText as percent } from "../bits";
import { api } from "../api";
import { DrawingFrame } from "./DrawingFrame";
import { guessNode, namesIn } from "../formula";

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
/** Where stepping through the worlds is: what it started from, how many there were, which it's at, and
 *  the world that is, to tell when something else has changed it. */
export type Stepping = { base: World; total: bigint; at: bigint; shown: string };
type Steps = [Stepping | null, (s: Stepping | null) => void];

/** Stepping, kept by whatever outlives the panel, so folding it away and back carries on where it was,
 *  and forgotten once the world is changed some other way. */
export function useStepping(ws: Workspace): Steps {
  const [stepping, setStepping] = useState<Stepping | null>(null);
  useEffect(() => {
    if (stepping && same(ws.world) !== stepping.shown) setStepping(null);
  }, [ws.world]); // eslint-disable-line react-hooks/exhaustive-deps
  return [stepping, setStepping];
}

/** A world as text that doesn't depend on the order its choices were listed in. */
const same = (world: World) => JSON.stringify(Object.keys(world).sort().map((k) => [k, world[k]]));

export function WorldPanel({ ws, onSelect, summary, steps }: { ws: Workspace; onSelect: (id: string | null) => void; summary: ReactNode; steps: Steps }) {
  const g = ws.view!;
  const w = g.worlds;
  const quantities = w.quantities ?? [];
  const pinned = Object.keys(w.picks).length + quantities.length + Object.keys(ws.world).filter((k) => k.startsWith("!")).length + (ws.lookThrough ? 1 : 0);
  // What it all means, for whoever asks.
  const [help, setHelp] = useState(false);
  // Each world is asked for by where it comes, so there's no limit to how far stepping goes.
  const [stepping, setStepping] = steps;
  const total = /^\d+$/.test(w.count) ? BigInt(w.count) : null;
  const step = async (by: number) => {
    const base = stepping?.base ?? ws.world;
    const n = stepping?.total ?? total;
    if (!n || n < BigInt(1)) return;
    const at = stepping ? (stepping.at + BigInt(by) + n) % n : by > 0 ? BigInt(0) : n - BigInt(1);
    const found = await api.worldAt(base, at.toString(), ws.where);
    if (!found) return;
    // What's ruled out, and quantities pinned, stay as they were: the worlds stepped through respect them.
    const next = { ...found, ...Object.fromEntries(Object.entries(base).filter(([k]) => k.startsWith("!") || k.startsWith("="))) };
    setStepping({ base, total: n, at, shown: same(next) });
    ws.setWorld(next);
  };
  const solutions = (stepping?.total ?? total ?? BigInt(0)).toLocaleString();
  // A puzzle's worlds are its solutions; anywhere else they're just worlds.
  const kind = w.constrained ? ["Solution", "solutions", "solution"] : ["World", "worlds", "world"];

  const drawings = g.nodes.filter((n) => typeof n.cells.drawing?.value === "string");
  const [wanted, setWanted] = useRemembered(`recollect.drawing:${g.workspace.id}`);
  const drawing = drawings.find((n) => n.id === wanted);
  const [failed, setFailed] = useState<string | null>(null);
  useEffect(() => setFailed(null), [drawing?.id]);
  const nameOf = (id: string) => ws.byId.get(id)?.name ?? "something";

  const options = w.vars.flatMap((v) => v.options.map((o) => o.name));
  const shared = w.vars.length > 0 && new Set(options).size === w.vars[0].options.length && w.vars.every((v) => v.options.length === w.vars[0].options.length);
  const isBoard = w.vars.length > 0 && w.vars.every((v) => placeOf(ws, v.id));

  return (
    <div className="world-panel">
      <div className="world-body">
      {/* The chip it grew from stays at the right end, where it was, with what acts on it before it. */}
      <div className="worlds-head">
        <span className="worlds-tools left">
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
              ws.setLookThrough("");
            }}
          >
            ⟲
          </button>
        </span>
        {summary}
      </div>
      <LookThrough ws={ws} />
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
            Every way the open choices could land is a world. Click an option to pin it, and again to let it go. Right-click one, or press and hold, to rule it out
            instead, and again to bring it back. Every page then shows what's true if so, and nothing is saved. Solid means pinned, a dot means it's the only one left,
            a cross means it's ruled out, and blank means no world lands there.
          </p>
          {w.vars.some((v) => v.uncertain) && <p>For what's uncertain, the percentage is the belief in each: how strongly it's believed given the evidence and whatever's pinned, so it changes as you pin things.</p>}
          <p>The arrows at the bottom step through the worlds left one at a time, each with every choice pinned.{w.constrained ? " Requirements rule some worlds out, so here the worlds left are the solutions." : ""}</p>
        </div>
      )}
      </div>
      {/* At the bottom of the card, which never gets shorter while it's open, so stepping never moves
          it, and the ? across from it. */}
      <div className="world-foot">
        {(stepping || (total !== null && total > BigInt(1))) && (
          <span className="stepper" title={stepping ? `${kind[0]} ${(stepping.at + BigInt(1)).toLocaleString()} of ${solutions}` : `Step through the ${solutions} ${kind[1]}, one at a time`}>
            <button className="icon small" onClick={() => step(-1)} aria-label={`Previous ${kind[2]}`}>
              ◀
            </button>
            <span className="stepper-text">{stepping ? `${kind[0]} ${(stepping.at + BigInt(1)).toLocaleString()} of ${solutions}` : `Step through ${solutions} ${kind[1]}`}</span>
            <button className="icon small" onClick={() => step(1)} aria-label={`Next ${kind[2]}`}>
              ▶
            </button>
          </span>
        )}
        <button className={`icon worlds-help ${help ? "on" : ""}`} aria-pressed={help} aria-label="What this means" title="What this means" onClick={() => setHelp(!help)}>
          ?
        </button>
      </div>
    </div>
  );
}

/** How the world says an option is ruled out, just for looking. The engine reads the same. */
const outKey = (v: VarView, o: OptionView) => `!${v.id}/${o.id}`;
export const isOut = (ws: ViewProps["ws"], v: VarView, o: OptionView) => outKey(v, o) in ws.world;

/** Click pins an option, or lets it go. On one that's ruled out, it brings it back instead. */
function pinner(ws: ViewProps["ws"], v: VarView, o: OptionView) {
  return () => (isOut(ws, v, o) ? ruler(ws, v, o)() : ws.pin(v.id, o.picked ? null : o.id));
}

/** Right-click (or a long press) rules an option out, or brings it back: only for looking, like pinning. */
function ruler(ws: ViewProps["ws"], v: VarView, o: OptionView) {
  return (e?: React.MouseEvent) => {
    e?.preventDefault();
    const next = { ...ws.world };
    const k = outKey(v, o);
    if (k in next) delete next[k];
    else {
      next[k] = "out";
      if (next[v.id] === o.id) delete next[v.id];
    }
    ws.setWorld(next);
  };
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
                  <td key={h.id} className={`world-cell ${o.picked ? "picked" : ""} ${onlyLeft(v, o) ? "forced" : ""} ${o.possible ? "possible" : "gone"} ${isOut(ws, v, o) ? "out" : ""}`}>
                    <button onClick={pinner(ws, v, o)} onContextMenu={ruler(ws, v, o)} aria-label={`${v.name}: ${o.name}`} title={`${v.name}: ${o.name}${isOut(ws, v, o) ? ", ruled out" : ""}`}>
                      {o.picked ? "●" : isOut(ws, v, o) ? "×" : onlyLeft(v, o) ? "•" : ""}
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

type Place = { row: number; col: number; byKey: boolean };

/**
 * Where something goes on a board: where its `at` says, counting from 0, or, for a row of a grid numbered
 * both ways, as `Cell[2, 3]`, where its key says.
 */
function placeOf(ws: ViewProps["ws"], id: string): Place | undefined {
  const n = ws.byId.get(id);
  const at = n?.cells.at?.value as { row: number; col: number } | undefined;
  if (at) return { ...at, byKey: false };
  const key = n?.row?.key;
  if (Array.isArray(key) && key.length === 2 && typeof key[0] === "number" && typeof key[1] === "number") return { row: key[0], col: key[1], byKey: true };
  return undefined;
}

/**
 * Choices placed on a board, each showing what's still possible where it is. A cell of the same grid that's
 * given, a choice of one option, isn't open, so it isn't one of the choices: it's shown where it is, fixed.
 */
function Board({ ws }: { ws: ViewProps["ws"] }) {
  const g = ws.view!;
  const w = g.worlds;
  const open = new Set(w.vars.map((v) => v.id));
  const tables = new Set(w.vars.map((v) => ws.byId.get(v.id)?.row?.table).filter((t) => t !== undefined));
  const givens = [...g.nodes, ...g.unwritten]
    .filter((n) => !open.has(n.id) && n.row && tables.has(n.row.table) && placeOf(ws, n.id)?.byKey)
    .flatMap((n) => {
      const options = n.cells.options?.value;
      return Array.isArray(options) && options.length === 1 ? [{ id: n.id, name: n.name, value: String(options[0]) }] : [];
    });
  // A grid's keys start where its extent does, as 1: its first row and column are the board's.
  const keyed = [...w.vars.map((v) => placeOf(ws, v.id)!), ...givens.map((x) => placeOf(ws, x.id)!)].filter((p) => p.byKey);
  const top = keyed.length ? Math.min(...keyed.map((p) => p.row)) : 0;
  const left = keyed.length ? Math.min(...keyed.map((p) => p.col)) : 0;
  const at = (id: string) => {
    const p = placeOf(ws, id)!;
    return p.byKey ? { row: p.row - top, col: p.col - left } : p;
  };
  const all = [...w.vars.map((v) => at(v.id)), ...givens.map((x) => at(x.id))];
  const rows = Math.max(...all.map((p) => p.row)) + 1;
  const cols = Math.max(...all.map((p) => p.col)) + 1;
  // A big board's cells are smaller, so a 9×9 fits.
  const size = Math.max(rows, cols) > 5 ? 52 : 88;
  return (
    <div className="board" style={{ gridTemplateColumns: `repeat(${cols}, ${size}px)`, gridTemplateRows: `repeat(${rows}, ${size}px)` }}>
      {givens.map((x) => {
        const { row, col } = at(x.id);
        return (
          <div key={x.id} className="board-cell given" style={{ gridRow: row + 1, gridColumn: col + 1 }} title={`${x.name}: given`}>
            <span className="board-big">{x.value}</span>
          </div>
        );
      })}
      {w.vars.map((v) => {
        const { row, col } = at(v.id);
        const side = Math.ceil(Math.sqrt(v.options.length));
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
              <span className="board-small" style={{ gridTemplateColumns: `repeat(${side}, 1fr)`, gridTemplateRows: `repeat(${side}, 1fr)` }}>
                {v.options.map((o) => (
                  <button key={o.id} className={isOut(ws, v, o) ? "out" : ""} disabled={!o.possible && !isOut(ws, v, o)} onClick={pinner(ws, v, o)} onContextMenu={ruler(ws, v, o)}>
                    {o.possible || isOut(ws, v, o) ? o.name : ""}
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
              <button
                key={o.id}
                className={`world-opt ${o.picked ? "picked" : ""} ${o.possible ? "" : "gone"} ${o.gap ? "gap" : ""} ${isOut(ws, v, o) ? "out" : ""}`}
                onClick={pinner(ws, v, o)}
                onContextMenu={ruler(ws, v, o)}
                title={isOut(ws, v, o) ? `${o.name}: ruled out. Click to bring it back` : `${o.name}. Right-click to rule it out`}
              >
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
  // What's believed most, which may be several, or everything there is, before anything's found out.
  const most = Math.max(...target.options.map((o) => o.chance));
  const likeliest = target.options.filter((o) => o.chance > 0 && Math.abs(o.chance - most) < 1e-9);
  const names = (os: OptionView[]) => (os.length === 1 ? optionName(ws, os[0]) : `${os.slice(0, -1).map((o) => optionName(ws, o)).join(", ")} and ${optionName(ws, os[os.length - 1])}`);
  const believed = !likeliest.length
    ? "No world is left for any of them."
    : likeliest.length === target.options.filter((o) => o.chance > 0).length && likeliest.length > 1
      ? `Nothing is believed more than anything else yet: ${likeliest.length === 2 ? "both" : "each"} ${percent(most)}.`
      : likeliest.length > 1
        ? `Believed most now: ${names(likeliest)}, ${percent(most)} each.`
        : `Believed most now: ${names(likeliest)}, ${percent(most)}.`;
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
        {believed}
        {help && target.clues?.length ? " Each of these, once known, would leave you this sure on average." : ""}
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

/** What a view leans towards: every decision that leans landing where it leans. */
export const CURRENT_INTENT = "all(d in with.leaning: d.chosen = d.leaning)";

/**
 * A formula to look through as well as what's pinned: only the worlds where it's true are shown, as a read
 * with `where` does. It's typed, or one click for current intent where something leans. Nothing is saved.
 * A name in it means a node: the one called that, ignoring case and with _ for a space, where that's
 * clear, and otherwise it asks which.
 */
function LookThrough({ ws }: { ws: Workspace }) {
  const [text, setText] = useState(ws.lookThrough);
  const [labels, setLabels] = useState(ws.lookLabels);
  useEffect(() => setText(ws.lookThrough), [ws.lookThrough]);
  useEffect(() => setLabels(ws.lookLabels), [ws.lookLabels]);
  const leans = ws.view!.worlds.vars.some((v) => v.leaning);
  const nodes = ws.view!.nodes;
  const names = namesIn(text);
  // What each name means: what was said, or else the node it plainly is.
  const meant = (given: Record<string, string>) =>
    Object.fromEntries(names.flatMap((n) => {
      const id = given[n] ?? guessNode(nodes, n)?.id;
      return id ? [[n, id]] : [];
    }));
  const set = (f: string, given = labels) => {
    const formula = f.trim();
    const now = formula === text.trim() ? meant(given) : {};
    setLabels(now);
    ws.setLookThrough(formula, now);
  };
  const shown = meant(labels);
  // A name nobody has said the node for is what's wrong, whatever the formula makes of it.
  const unsaid = names.filter((n) => !shown[n]);
  return (
    <div className="look-through">
      <label className="small muted" htmlFor="look-through">
        Look only where
      </label>
      <input
        id="look-through"
        className={`small ${ws.lookError ? "bad" : ""}`}
        value={text}
        placeholder="a true-or-false formula"
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") set(text);
          if (e.key === "Escape") setText(ws.lookThrough);
        }}
        onBlur={() => set(text)}
        title="Only the worlds where this is true are shown, and nothing is saved. Where it can't be told, a world is kept."
      />
      {leans && ws.lookThrough !== CURRENT_INTENT && (
        <button className="link small" title="Only the worlds where every decision that leans lands where it leans" onClick={() => set(CURRENT_INTENT)}>
          current intent
        </button>
      )}
      {ws.lookThrough && (
        <button className="icon small" aria-label="Stop looking through it" onClick={() => set("")}>
          ×
        </button>
      )}
      {names.length > 0 && (
        <div className="look-names">
          {names.map((n) => (
            <div key={n} className="formula-label small">
              <code>{n}</code>
              {shown[n] ? (
                <span className="edge-chip">
                  {ws.byId.get(shown[n])?.name ?? shown[n]}
                  <button
                    className="icon small"
                    title={`${n} isn't this: say which node it is`}
                    aria-label={`${n} isn't this`}
                    onClick={() => {
                      const { [n]: _, ...rest } = labels;
                      // Asked for, so not guessed again until another is picked.
                      setLabels({ ...rest, [n]: "" });
                    }}
                  >
                    ×
                  </button>
                </span>
              ) : (
                <input
                  list="look-nodes"
                  className="small"
                  aria-label={`What ${n} refers to`}
                  placeholder="which node?"
                  onChange={(e) => {
                    const found = nodes.find((x) => x.name === e.target.value);
                    if (found) set(text, { ...labels, [n]: found.id });
                  }}
                />
              )}
            </div>
          ))}
          <datalist id="look-nodes">
            {nodes.map((x) => (
              <option key={x.id} value={x.name} />
            ))}
          </datalist>
        </div>
      )}
      {ws.lookError && <div className="error small">{unsaid.length ? `Say which node ${unsaid.join(" and ")} ${unsaid.length === 1 ? "means" : "mean"}.` : ws.lookError}</div>}
    </div>
  );
}
