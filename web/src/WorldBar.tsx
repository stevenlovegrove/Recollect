import type { Workspace } from "./workspace";
import { DdItemButton, Dropdown } from "./Dropdown";

const FEW = 5;

/** Which world everything on screen is narrowed to. It's the app's one what-if, and saves nothing. */
export function WorldBar({ ws, onShowWorlds }: { ws: Workspace; onShowWorlds: () => void }) {
  const w = ws.view!.worlds;
  if (!w.vars.length) return null;
  const pinned = Object.keys(w.picks).length;
  return (
    <div className={`world-bar ${pinned ? "narrowed" : ""}`}>
      <button className="world-bar-count" onClick={onShowWorlds} title="See every choice">
        <b>{w.count === "0" ? "No worlds" : Number(w.count).toLocaleString()}</b> {w.count === "1" ? "world" : w.count === "0" ? "fit" : "worlds"}
      </button>
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
                  {!o.gap && <span className="muted small">{Math.round(o.weight * 100)}%</span>}
                </DdItemButton>
              ))}
            </Dropdown>
          );
        })
      ) : (
        <span className="muted small">{pinned ? `${pinned} of ${w.vars.length} choices pinned` : `${w.vars.length} open choices`}</span>
      )}
      {pinned > 0 && (
        <button className="link" onClick={() => ws.setWorld({})}>
          clear
        </button>
      )}
      {pinned > 0 && <span className="muted small world-bar-note">Showing what's true if so. Nothing is saved.</span>}
    </div>
  );
}
