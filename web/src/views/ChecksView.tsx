import { useMemo } from "react";
import type { Check } from "../api";
import type { ViewProps } from "./MapView";
import { NodeChip } from "../bits";

const HELP: Record<string, string> = {
  no_deadlock: "Work that can never finish, because it waits on itself.",
  chosen_must_be_a_live_option: "A decision that landed on something that's no longer an option.",
  one_decomposition_per_link: "An edge that's both part of something and an option of it.",
  unknown_has_no_parts: "A gap that has parts: give them a real node instead.",
  unknown_cannot_be_done: "A gap marked done: remove it to close it.",
  unknown_cannot_be_chosen: "A decision that chose a gap: name the option instead.",
  unknown_component: "Something given a component no system here provides.",
  component_on_wrong_target: "A node given what goes on an edge, or an edge given what goes on a node.",
  cell_without_component: "A cell written on something without the component it belongs to.",
  malformed_cell: "A cell holding something of the wrong shape.",
  formula_cannot_be_read: "A formula that can't be read.",
  blocking_decision: "Safe work has run out: decide, or find work the options share.",
  decision_without_options: "An open decision with nothing left to choose from.",
  one_option_left: "A decision that isn't really one any more.",
  stated_done_with_open_work: "Marked done, while what's beneath it is still open. What it says for itself is what counts.",
  dropped_part_of_live_work: "Dropped from something still in play, which no longer requires it.",
  undeveloped_goal: "Goals with nothing decomposing or serving them yet.",
  risk_without_target: "Risks not linked to what they threaten.",
  oversized_task: "Big tasks with no breakdown.",
  leaning_must_be_a_live_option: "A decision leaning towards something that's no longer an option.",
  edge_of_the_wrong_kinds: "An edge whose ends aren't the kinds its relation says, or whose kind isn't a relation.",
  arranging_loop: "A relation that lays the map out, whose edges go round in a loop.",
  should_hold: "What the workspace says should be true, where it isn't.",
  cell_used_once: "A cell written once, a letter or two from another: perhaps a typo.",
  name_shared: "Nodes with the same name, which a name alone can't tell apart.",
  relates_without_a_kind: "Edges that say two things relate without saying how.",
  relation_used_once: "A relation with a single edge, which may be another by a different name.",
};

/** What the checks have to say. Anything broken got there by a hand edit, because a commit would refuse it. */
export function ChecksView({ ws, onSelect }: ViewProps) {
  const groups = useMemo(() => {
    const m = new Map<string, Check[]>();
    for (const c of ws.view!.checks) m.set(c.rule, [...(m.get(c.rule) ?? []), c]);
    // What's broken first, then what's worth a look, then what's only unresolved.
    const rank = (c: Check) => ({ broken: 0, notice: 1, unresolved: 2 })[c.severity];
    return [...m].sort((a, b) => rank(a[1][0]) - rank(b[1][0]) || b[1].length - a[1].length);
  }, [ws.view]);

  return (
    <div className="scroll pad">
      <div className="map-toolbar flat">
        <span className="muted">
          Checks worked out from the graph. Ask Claude to run <code>checks</code> and look beyond them.
        </span>
      </div>
      {groups.length === 0 && <p className="muted">Nothing to report.</p>}
      {groups.map(([rule, checks]) => (
        <section key={rule} className="gap-group">
          <h3>
            <span className={`sev sev-${checks[0].severity === "broken" ? "high" : checks[0].severity === "unresolved" ? "info" : "warn"}`}>{checks[0].severity === "broken" ? "broken" : checks[0].severity === "unresolved" ? "unresolved" : "worth a look"}</span> {rule.replace(/_/g, " ")} <span className="muted">({checks.length})</span>
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
