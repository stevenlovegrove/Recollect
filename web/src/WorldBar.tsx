import type { Workspace } from "./workspace";
import { DdItemButton, Dropdown } from "./Dropdown";
import { BELIEF_MEANS, beliefText } from "./bits";
import { ViewIcon } from "./ViewNav";

const FEW = 5;

function worldsOf(ws: Workspace) {
  const w = ws.view!.worlds;
  const quantities = w.quantities ?? [];
  const pinned = Object.keys(w.picks).length + quantities.length;
  return { w, quantities, pinned, any: w.vars.length > 0 || quantities.length > 0 };
}

function countText(count: string) {
  return (
    <>
      <b>{count === "0" ? "No worlds" : Number(count).toLocaleString()}</b> {count === "1" ? "world" : count === "0" ? "fit" : "worlds"}
    </>
  );
}

/** The choices and pins themselves: what the bar and the chip both hold. */
function WorldChoices({ ws }: { ws: Workspace }) {
  const { w, quantities, pinned } = worldsOf(ws);
  const nameOf = (id: string) => ws.byId.get(id)?.name ?? "something";
  return (
    <>
      {w.vars.length <= FEW ? (
        w.vars.map((v) => {
          const picked = v.options.find((o) => o.picked);
          return (
            <Dropdown key={v.id} active={!!picked} label={<>{v.name}: <b>{picked ? picked.name : "open"}</b></>}>
              <div className="dd-head">{v.name}</div>
              <DdItemButton onClick={() => ws.pin(v.id, null)}>
                <span className="dd-label">{picked ? "Leave it open" : "✓ Open"}</span>
              </DdItemButton>
              {v.options.map((o) => (
                <DdItemButton key={o.id} onClick={() => ws.pin(v.id, o.id)}>
                  <span className={`dd-label ${o.possible ? "" : "muted"}`}>
                    {o.picked ? "✓ " : ""}
                    {o.name}
                    {o.gap ? " (a gap)" : ""}
                  </span>
                  {v.uncertain && (o.chance > 0 || !o.gap) && (
                    <span className="muted small" title={`Belief: ${BELIEF_MEANS.toLowerCase()}. Prior belief, before any evidence: ${beliefText(o.weight)}.`}>
                      {beliefText(o.chance)}
                    </span>
                  )}
                </DdItemButton>
              ))}
            </Dropdown>
          );
        })
      ) : (
        <span className="muted small">{Object.keys(w.picks).length ? `${Object.keys(w.picks).length} of ${w.vars.length} choices pinned` : `${w.vars.length} open choices`}</span>
      )}
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
      {pinned > 0 && (
        <button className="link" onClick={() => ws.setWorld({})}>
          clear
        </button>
      )}
      {pinned > 0 && <span className="muted small world-bar-note">Showing what's true if so. Nothing is saved.</span>}
    </>
  );
}

/** Which world everything on screen is narrowed to. It's the app's one what-if, and saves nothing. */
export function WorldBar({ ws, onShowWorlds }: { ws: Workspace; onShowWorlds: () => void }) {
  const { w, pinned, any } = worldsOf(ws);
  if (!any) return null;
  return (
    <div className={`world-bar ${pinned ? "narrowed" : ""}`}>
      <button className="world-bar-count" onClick={onShowWorlds} title="See every choice">
        {countText(w.count)}
      </button>
      <WorldChoices ws={ws} />
    </div>
  );
}

/** The same, folded into a chip over the map, across from its view options. A dot says the map is
 *  narrowed to some worlds even while the chip is folded, so it never looks like all of them. */
export function WorldChip({ ws, open, onToggle, onShowWorlds }: { ws: Workspace; open: boolean; onToggle: () => void; onShowWorlds: () => void }) {
  const { w, pinned, any } = worldsOf(ws);
  if (!any) return null;
  return (
    <>
      <button className={`world-chip ${open ? "open" : ""} ${pinned ? "narrowed" : ""}`} onClick={onToggle} aria-expanded={open} title={open ? "Hide the worlds" : "Narrow to some worlds"}>
        <ViewIcon name="worlds" />
        <span className="world-chip-count">{countText(w.count)}</span>
      </button>
      {open && (
        <div className={`world-pop ${pinned ? "narrowed" : ""}`}>
          <WorldChoices ws={ws} />
          {w.vars.length > 0 && (
            <button className="link world-pop-all" onClick={onShowWorlds}>
              See every choice →
            </button>
          )}
        </div>
      )}
    </>
  );
}
