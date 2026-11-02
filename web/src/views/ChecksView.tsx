import { useMemo } from "react";
import type { Check } from "../api";
import type { ViewProps } from "./MapView";
import { NodeChip } from "../bits";

const HELP: Record<string, string> = {
  no_deadlock: "Work that can never finish, because it waits on itself.",
  chosen_must_be_a_live_option: "A decision that landed on something that's no longer an option.",
  one_decomposition_per_link: "A link that's both part of something and an option of it.",
  unknown_has_no_parts: "A gap that has parts: give them a real node instead.",
  unknown_cannot_be_done: "A gap marked done: remove it to close it.",
  unknown_cannot_be_chosen: "A decision that chose a gap: name the option instead.",
  link_target_lacks_component: "An option of something that isn't a decision.",
  blocking_decision: "Safe work has run out: decide, or find work the options share.",
  decision_without_options: "An open decision with nothing left to choose from.",
  one_option_left: "A decision that isn't really one any more.",
  stated_done_with_open_work: "Done only counts at a leaf: above that, it's worked out from what's beneath.",
  dropped_part_of_live_work: "Dropped from something still in play, which no longer requires it.",
  undeveloped_goal: "Goals with nothing decomposing or serving them yet.",
  risk_without_target: "Risks not linked to what they threaten.",
  oversized_task: "Big tasks with no breakdown.",
};

/** What the rules have to say. Anything broken got there by a hand edit, because a commit would refuse it. */
export function ChecksView({ ws, onSelect }: ViewProps) {
  const groups = useMemo(() => {
    const m = new Map<string, Check[]>();
    for (const c of ws.view!.checks) m.set(c.rule, [...(m.get(c.rule) ?? []), c]);
    return [...m].sort((a, b) => Number(a[1][0].severity === "notice") - Number(b[1][0].severity === "notice") || b[1].length - a[1].length);
  }, [ws.view]);

  return (
    <div className="scroll pad">
      <div className="map-toolbar flat">
        <span className="muted">
          Rules worked out from the graph. Ask Claude to run <code>find_checks</code> and look beyond them.
        </span>
      </div>
      {groups.length === 0 && <p className="muted">Nothing to report.</p>}
      {groups.map(([rule, checks]) => (
        <section key={rule} className="gap-group">
          <h3>
            <span className={`sev sev-${checks[0].severity === "broken" ? "high" : "warn"}`}>{checks[0].severity === "broken" ? "broken" : "worth a look"}</span> {rule.replace(/_/g, " ")} <span className="muted">({checks.length})</span>
          </h3>
          {HELP[rule] && <p className="muted small">{HELP[rule]}</p>}
          <ul className="gap-list">
            {checks.map((c, i) => (
              <li key={i}>
                {c.on.map((n) => (
                  <NodeChip key={n} ws={ws} id={n} onSelect={onSelect} />
                ))}
                <span className="gap-msg">{c.detail}</span>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
