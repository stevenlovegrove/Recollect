//! Crashing at every point a commit or a repair can be interrupted. Each crash is a real process
//! dying at that step, and the survivor has to find either all of the commit or none of it.
//!
//! That covers a process dying, and not power loss, where what's been flushed matters too: nothing
//! here can show the disk honoured a flush.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use recollect_graph::log::CRASHED;
use recollect_systems::plan::{self, Decision, Done, Estimate, PartOf, Task};
use recollect_graph::{Entity, LoadProblem, Log, Op, Registry, SavedGraph, Stored};

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    reg.build("crash-test");
    Arc::new(reg)
}

fn id(n: u128) -> Entity {
    uuid_like(n).parse().unwrap()
}

fn uuid_like(n: u128) -> String {
    format!("{:08x}-0000-7000-8000-{:012x}", 1, n)
}

/// A graph with several files, and a commit that changes some, creates one, removes one and changes
/// `graph.yaml`.
fn setup(dir: &Path) -> SavedGraph {
    let (mut log, _) = Log::open(dir, registry(), "ext").unwrap();
    log.commit("t", "setup", vec![Op::set(id(1), Task {}), Op::set(id(1), Estimate::days(1.0)), Op::set(id(2), Task {}), Op::set(id(3), Task {}), Op::set(id(4), Decision::default()), Op::link(id(2), id(1), PartOf)]).unwrap();
    log.graph().save().unwrap()
}

fn the_commit() -> Vec<Op> {
    vec![
        Op::set(id(1), Done),
        Op::remove::<Task>(id(2)),
        Op::unlink::<PartOf>(id(2), id(1)),
        Op::Despawn { id: id(3) },
        Op::set(id(5), Task {}), Op::set(id(5), Estimate::days(2.0)),
        Op::link(id(5), id(2), PartOf),
        Op::Default { component: "marker".into(), to: Some(Stored::Value(serde_json::json!(7))) },
    ]
}

fn run_child(test: &str, dir: &Path, envs: &[(&str, String)]) -> i32 {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--ignored", "--exact", test, "--nocapture"]).env("CRASH_DIR", dir);
    for (k, v) in envs {
        c.env(k, v);
    }
    let output = c.output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    assert!(code == 0 || code == CRASHED, "{test} failed unexpectedly: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    code
}

/// Run by the tests below, in its own process, so it can die.
#[test]
#[ignore]
fn commit_then_die() {
    let dir = std::env::var("CRASH_DIR").expect("run by a test in this file");
    let steps: u32 = std::env::var("STEPS").unwrap().parse().unwrap();
    let (mut log, _) = Log::open(Path::new(&dir), registry(), "ext").unwrap();
    log.crash_after_steps(steps);
    log.commit("t", "the commit", the_commit()).unwrap();
}

/// Run by the tests below, in its own process, so it can die.
#[test]
#[ignore]
fn checkout_then_die() {
    let dir = std::env::var("CRASH_DIR").expect("run by a test in this file");
    let steps: u32 = std::env::var("STEPS").unwrap().parse().unwrap();
    let (mut log, _) = Log::open(Path::new(&dir), registry(), "ext").unwrap();
    log.crash_after_steps(steps);
    log.checkout(0, "t", "going back").unwrap();
}

#[test]
fn a_crash_at_any_step_of_going_back_leaves_it_there_or_where_it_was() {
    let (base, before, after) = expected_states();
    {
        let (mut log, _) = Log::open(base.path(), registry(), "ext").unwrap();
        log.commit("t", "the commit", the_commit()).unwrap();
    }
    let mut outcomes = Vec::new();
    for steps in 0.. {
        let run = tempfile::tempdir().unwrap();
        copy_dir(base.path(), run.path());
        let code = run_child("checkout_then_die", run.path(), &[("STEPS", steps.to_string())]);
        let (mut log, problems) = Log::open(run.path(), registry(), "ext").unwrap();
        for p in &problems {
            assert!(matches!(p, LoadProblem::TornLog { .. }), "a crash isn't an edit: {p:?}");
        }
        assert_eq!(log.events().len(), 2, "going back adds no events");
        let now = log.graph().save().unwrap();
        let went = log.last_seq() == Some(0);
        assert_eq!(now, if went { before.clone() } else { after.clone() }, "at step {steps}");
        let (plain, _) = recollect_graph::Graph::load_dir(run.path(), registry()).unwrap();
        assert_eq!(plain.save().unwrap(), now, "the files have caught up at step {steps}");
        outcomes.push(went);
        // And it goes on working, as a branch if it did go back.
        let seq = log.commit("t", "afterwards", vec![Op::set(id(6), Done)]).unwrap();
        assert_eq!(log.events()[seq as usize].parent, Some(if went { 0 } else { 1 }));
        if code == 0 {
            assert!(steps > 5, "this should have passed several steps: {steps}");
            break;
        }
    }
    assert!(outcomes.contains(&false) && outcomes.contains(&true), "{outcomes:?}");
}

/// Run by the tests below, in its own process, so it can die.
#[test]
#[ignore]
fn open_then_die() {
    let dir = std::env::var("CRASH_DIR").expect("run by a test in this file");
    let steps: u32 = std::env::var("STEPS").unwrap().parse().unwrap();
    Log::open_crashing_after(Path::new(&dir), registry(), "ext", steps).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The state a crash at this step left, opened as the next run would. Anything but a whole commit
/// or none of it, or anything recorded as an edit by someone else, fails here.
fn recover_and_check(dir: &Path, before: &SavedGraph, after: &SavedGraph) -> bool {
    let (mut log, problems) = Log::open(dir, registry(), "ext").unwrap();
    for p in &problems {
        assert!(matches!(p, LoadProblem::TornLog { .. }), "a crash isn't an edit: {p:?}");
    }
    assert_eq!(log.violations(), vec![]);
    let now = log.graph().save().unwrap();
    let committed = now == *after;
    assert!(committed || now == *before, "neither all of it nor none of it: {now:#?}");
    let events = if committed { 2 } else { 1 };
    assert_eq!(log.events().len(), events);

    // The files say the same thing, and the log says how far they've got.
    let (plain, _) = recollect_graph::Graph::load_dir(dir, registry()).unwrap();
    assert_eq!(plain.save().unwrap(), now, "the files have caught up");
    assert_eq!(std::fs::read_to_string(dir.join("log.applied")).unwrap().trim(), (events - 1).to_string());
    let leftovers: Vec<_> = std::fs::read_dir(dir.join("nodes")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".tmp")).collect();
    assert_eq!(leftovers, Vec::<String>::new());

    // And it goes on working.
    log.commit("t", "afterwards", vec![Op::set(id(6), Done)]).unwrap();
    drop(log);
    let (log, problems) = Log::open(dir, registry(), "ext").unwrap();
    assert_eq!(problems, vec![], "a clean run leaves nothing to report");
    assert_eq!(log.events().len(), events + 1);
    committed
}

fn expected_states() -> (tempfile::TempDir, SavedGraph, SavedGraph) {
    let base = tempfile::tempdir().unwrap();
    let before = setup(base.path());
    let after = {
        let copy = tempfile::tempdir().unwrap();
        copy_dir(base.path(), copy.path());
        let (mut log, _) = Log::open(copy.path(), registry(), "ext").unwrap();
        log.commit("t", "the commit", the_commit()).unwrap();
        log.graph().save().unwrap()
    };
    (base, before, after)
}

#[test]
fn a_crash_at_any_step_of_a_commit_leaves_all_of_it_or_none_of_it() {
    let (base, before, after) = expected_states();
    let mut outcomes = Vec::new();
    for steps in 0.. {
        let run = tempfile::tempdir().unwrap();
        copy_dir(base.path(), run.path());
        let code = run_child("commit_then_die", run.path(), &[("STEPS", steps.to_string())]);
        if code == 0 {
            assert!(steps > 15, "this should have passed many steps: {steps}");
            break;
        }
        outcomes.push(recover_and_check(run.path(), &before, &after));
    }
    let happened = outcomes.iter().position(|c| *c).expect("crashing late keeps the commit");
    assert!(happened >= 1, "crashing early loses it");
    assert!(outcomes[happened..].iter().all(|c| *c), "once it's happened, it stays happened: {outcomes:?}");
    println!("{} crash points, committed from step {happened}", outcomes.len());
}

#[test]
fn a_crash_while_repairing_after_a_crash_changes_nothing() {
    let (base, before, after) = expected_states();
    let (mut repairs, mut last_commit_step) = (0, None);
    // Every interesting place to have died, then every place to die while repairing it.
    for steps in (0..).step_by(3) {
        let first = tempfile::tempdir().unwrap();
        copy_dir(base.path(), first.path());
        if run_child("commit_then_die", first.path(), &[("STEPS", steps.to_string())]) == 0 {
            last_commit_step = Some(steps);
            break;
        }
        for repair_steps in (0..).step_by(4) {
            let second = tempfile::tempdir().unwrap();
            copy_dir(first.path(), second.path());
            repairs += 1;
            if run_child("open_then_die", second.path(), &[("STEPS", repair_steps.to_string())]) == 0 {
                recover_and_check(second.path(), &before, &after);
                break;
            }
            recover_and_check(second.path(), &before, &after);
        }
    }
    assert!(last_commit_step.is_some());
    assert!(repairs > 10, "{repairs}");
    println!("{repairs} crashes during repair");
}

#[test]
fn a_torn_line_at_any_length_is_cut_off() {
    let (base, before, _) = expected_states();
    let line_source = tempfile::tempdir().unwrap();
    copy_dir(base.path(), line_source.path());
    {
        let (mut log, _) = Log::open(line_source.path(), registry(), "ext").unwrap();
        log.commit("t", "the commit", the_commit()).unwrap();
    }
    let old = std::fs::read(base.path().join("log.jsonl")).unwrap();
    let new = std::fs::read(line_source.path().join("log.jsonl")).unwrap();
    assert!(new.starts_with(&old));
    let line = &new[old.len()..];
    assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);

    // Every prefix short of the newline, which is the moment it counts.
    for len in (1..line.len()).step_by(7).chain([line.len() - 1]) {
        let run = tempfile::tempdir().unwrap();
        copy_dir(base.path(), run.path());
        let mut torn = old.clone();
        torn.extend_from_slice(&line[..len]);
        std::fs::write(run.path().join("log.jsonl"), &torn).unwrap();

        let (log, problems) = Log::open(run.path(), registry(), "ext").unwrap();
        assert_eq!(problems, vec![LoadProblem::TornLog { bytes: len as u64 }]);
        assert_eq!(log.graph().save().unwrap(), before);
        drop(log);
        assert_eq!(std::fs::read(run.path().join("log.jsonl")).unwrap(), old, "and it's gone");
        let (_, problems) = Log::open(run.path(), registry(), "ext").unwrap();
        assert_eq!(problems, vec![]);
    }
}

#[test]
fn a_corrupt_line_in_the_middle_is_an_error_not_a_quiet_loss() {
    let (base, _, _) = expected_states();
    let text = std::fs::read_to_string(base.path().join("log.jsonl")).unwrap();
    std::fs::write(base.path().join("log.jsonl"), format!("not json\n{text}")).unwrap();
    assert!(matches!(Log::open(base.path(), registry(), "ext"), Err(recollect_graph::OpenError::Corrupt { line: 1, .. })));
}

#[test]
fn a_hand_edit_made_while_the_files_were_behind_still_stands() {
    let (base, _, after) = expected_states();
    // Find a crash where the commit happened and the files haven't caught up.
    for steps in 0.. {
        let run = tempfile::tempdir().unwrap();
        copy_dir(base.path(), run.path());
        if run_child("commit_then_die", run.path(), &[("STEPS", steps.to_string())]) == 0 {
            panic!("no such crash found");
        }
        let log_text = std::fs::read_to_string(run.path().join("log.jsonl")).unwrap();
        let applied = std::fs::read_to_string(run.path().join("log.applied")).unwrap();
        if log_text.lines().filter(|l| l.contains("\"record\":\"event\"")).count() != 2 || applied.trim() != "0" {
            continue;
        }

        // Someone edits a file the commit never touched.
        let untouched = run.path().join("nodes").join(format!("{}.yaml", id(4)));
        let text = std::fs::read_to_string(&untouched).unwrap();
        assert!(text.contains("decision: {}"), "{text}");
        std::fs::write(&untouched, text.replace("decision: {}", "decision: {}\n  note: by hand")).unwrap();

        let (log, problems) = Log::open(run.path(), registry(), "by hand").unwrap();
        assert_eq!(problems, vec![LoadProblem::ExternalEdit { seq: 2 }], "the commit isn't an edit, and this is");
        let mut expected = after.clone();
        expected.nodes.iter_mut().find(|n| n.id == id(4)).unwrap().components.insert("note".into(), serde_json::json!("by hand"));
        assert_eq!(log.graph().save().unwrap(), expected, "both are there");
        assert_eq!(log.events()[1].why, "the commit");
        assert_eq!(log.events()[2].ops.len(), 1, "and the edit is only the edit: {:?}", log.events()[2].ops);
        return;
    }
}

#[test]
fn a_failed_write_of_the_log_changes_nothing_and_stale_files_are_repaired() {
    // A directory that can't be written stands in for a full disk.
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = Log::open(dir.path(), registry(), "ext").unwrap();
    log.commit("t", "first", vec![Op::set(id(1), Task {})]).unwrap();

    let nodes = dir.path().join("nodes");
    let mut perms = std::fs::metadata(&nodes).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&nodes, perms.clone()).unwrap();
    let result = log.commit("t", "second", vec![Op::set(id(2), Done)]);
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&nodes, perms).unwrap();

    assert!(matches!(result, Err(recollect_graph::CommitError::FilesBehind { seq: 1, .. })), "{result:?}");
    assert_eq!(log.graph().get(id(2)), Ok(Some(Done)), "it did happen");
    log.commit("t", "third", vec![Op::set(id(1), Done)]).unwrap();
    let saved = log.graph().save().unwrap();
    drop(log);

    let (plain, _) = recollect_graph::Graph::load_dir(dir.path(), registry()).unwrap();
    assert_eq!(plain.save().unwrap(), saved, "the next commit caught the files up");
    let (log, problems) = Log::open(dir.path(), registry(), "ext").unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(log.graph().save().unwrap(), saved);
}
