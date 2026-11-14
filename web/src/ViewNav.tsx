import { useEffect, useState, type ReactNode } from "react";
import { label } from "./theme";

/** How the views are reached on a narrow screen: a bar along the bottom with the main ones, and
 *  a sheet with all of them. On a wide screen they're the tabs in the top bar instead. */

const P = { fill: "none", stroke: "currentColor", strokeWidth: 1.7, strokeLinecap: "round", strokeLinejoin: "round" } as const;

const ICONS: Record<string, ReactNode> = {
  map: (
    <>
      <circle cx="12" cy="5" r="2.5" {...P} />
      <circle cx="5" cy="18" r="2.5" {...P} />
      <circle cx="19" cy="18" r="2.5" {...P} />
      <path d="M10.7 7.2 6.3 15.8M13.3 7.2l4.4 8.6M7.5 18h9" {...P} />
    </>
  ),
  outline: <path d="M4 5h16M8 10h12M8 15h12M4 20h16M4 10v5" {...P} />,
  table: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" {...P} />
      <path d="M3.5 9.5h17M3.5 14.5h17M10 4.5v15" {...P} />
    </>
  ),
  worlds: (
    <>
      <circle cx="12" cy="12" r="8.5" {...P} />
      <path d="M3.5 12h17M12 3.5c-3 3-3 14 0 17M12 3.5c3 3 3 14 0 17" {...P} />
    </>
  ),
  decisions: <path d="M12 21v-7M12 14 6 8M12 14l6-6M6 8V4M6 4 3.5 6.5M6 4l2.5 2.5M18 8V4M18 4l-2.5 2.5M18 4l2.5 2.5" {...P} />,
  work: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" {...P} />
      <path d="m8.5 12 2.5 2.5 4.5-5" {...P} />
    </>
  ),
  timeline: <path d="M4 6h9M8 12h10M6 18h8M3 3v18" {...P} />,
  checks: (
    <>
      <path d="M12 3.5 21 19.5H3z" {...P} />
      <path d="M12 10v4M12 16.8v.2" {...P} />
    </>
  ),
  history: (
    <>
      <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3L4 9.4" {...P} />
      <path d="M4 4.5v4.9h4.9M12 8v4.5l3 2" {...P} />
    </>
  ),
  files: (
    <>
      <path d="M6 3.5h8l4.5 4.5v12.5H6z" {...P} />
      <path d="M14 3.5V8h4.5M9 13h6M9 16.5h6" {...P} />
    </>
  ),
  more: (
    <>
      <circle cx="5" cy="12" r="1.6" fill="currentColor" />
      <circle cx="12" cy="12" r="1.6" fill="currentColor" />
      <circle cx="19" cy="12" r="1.6" fill="currentColor" />
    </>
  ),
};

export function ViewIcon({ name }: { name: string }) {
  return (
    <svg className="view-icon" viewBox="0 0 24 24" width="22" height="22" aria-hidden>
      {ICONS[name] ?? ICONS.more}
    </svg>
  );
}

type Props<T extends string> = {
  tabs: T[];
  tab: T;
  onTab: (t: T) => void;
  counts: Partial<Record<T, number>>;
  hot: (t: T) => boolean;
};

/** The views that earn a place in the bar: the three every workspace has, and the one its
 *  systems make most worth reaching. */
function pinned<T extends string>(tabs: T[]): T[] {
  const first = (["map", "outline", "table"] as string[]).filter((t) => tabs.includes(t as T)) as T[];
  const fourth = (["decisions", "worlds", "timeline"] as string[]).find((t) => tabs.includes(t as T)) as T | undefined;
  return fourth ? [...first, fourth] : first;
}

export function BottomNav<T extends string>({ tabs, tab, onTab, counts, hot }: Props<T>) {
  const [more, setMore] = useState(false);
  const bar = pinned(tabs);
  const rest = tabs.filter((t) => !bar.includes(t));
  const inRest = rest.includes(tab);
  // What's hidden behind "More" and wants attention, so the bar can say so.
  const restCount = rest.filter((t) => hot(t)).reduce((n, t) => n + (counts[t] ?? 0), 0);
  const restHot = restCount > 0;

  // The sheet is for choosing: once a view is chosen, it goes.
  useEffect(() => setMore(false), [tab]);

  return (
    <>
      <nav className="bottom-nav" aria-label="Views">
        {bar.map((t) => (
          <button key={t} className={t === tab ? "bn-item active" : "bn-item"} onClick={() => onTab(t)} aria-current={t === tab ? "page" : undefined}>
            <span className="bn-icon">
              <ViewIcon name={t} />
              {counts[t] ? <span className={hot(t) ? "bn-badge hot" : "bn-badge"}>{counts[t]}</span> : null}
            </span>
            <span className="bn-label">{label(t)}</span>
          </button>
        ))}
        <button className={inRest || more ? "bn-item active" : "bn-item"} onClick={() => setMore(!more)} aria-expanded={more}>
          <span className="bn-icon">
            <ViewIcon name={inRest ? tab : "more"} />
            {restHot ? <span className="bn-badge hot">{restCount}</span> : null}
          </span>
          <span className="bn-label">{inRest ? label(tab) : "More"}</span>
        </button>
      </nav>
      {more && (
        <div className="views-backdrop" onClick={() => setMore(false)}>
          <div className="views-sheet" role="dialog" aria-label="All views" onClick={(e) => e.stopPropagation()}>
            <div className="views-grip" />
            <div className="views-grid">
              {tabs.map((t) => (
                <button key={t} className={t === tab ? "views-item active" : "views-item"} onClick={() => onTab(t)}>
                  <span className="bn-icon">
                    <ViewIcon name={t} />
                    {counts[t] ? <span className={hot(t) ? "bn-badge hot" : "bn-badge"}>{counts[t]}</span> : null}
                  </span>
                  <span className="bn-label">{label(t)}</span>
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
