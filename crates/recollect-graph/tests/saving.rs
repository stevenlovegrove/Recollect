//! Rules that can be written down: registering them, the file format, and what happens when a
//! file and the running code don't line up. Nothing should ever be lost.

use std::fs;
use std::sync::Arc;

use recollect_graph::graph::Inputs;
use recollect_graph::{Compute, Cx, Entity, Error, Graph, LoadProblem, Name, Registry, RuntimeRule, SaveError, SavedRule, component, rule};
use serde_json::{Value, json};

#[component]
struct Amount(f64);

#[component]
struct Addend;

#[component]
struct Bonus;

#[rule("test/sum", incoming(Addend))]
struct Sum;

impl Compute for Sum {
    type Output = Amount;
    fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
        let mut total = 0.0;
        for from in cx.sources::<Addend>()? {
            total += cx.value::<Amount>(from)?.0;
        }
        Ok(Amount(total))
    }
}

/// With a parameter, which is what a closure would have captured.
#[rule("test/scaled", incoming(Addend))]
struct Scaled {
    by: f64,
}

impl Compute for Scaled {
    type Output = Amount;
    fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
        let mut total = 0.0;
        for from in cx.sources::<Addend>()? {
            total += cx.value::<Amount>(from)?.0;
        }
        Ok(Amount(total * self.by))
    }
}

/// What it follows depends on its parameters.
#[rule("test/sum_over")]
struct SumOver {
    kind: String,
}

impl Compute for SumOver {
    type Output = Amount;
    fn inputs(&self) -> Inputs {
        Inputs { incoming: vec![self.kind.clone()], ..Inputs::default() }
    }
    fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
        let mut total = 0.0;
        for (from, _) in cx.incoming_runtime(&self.kind)? {
            total += cx.value::<Amount>(from)?.0;
        }
        Ok(Amount(total))
    }
}

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    reg.rule::<Sum>().rule::<Scaled>().rule::<SumOver>();
    Arc::new(reg)
}

/// a + b = total
fn sheet() -> (Graph, Entity, Entity, Entity) {
    let mut g = Graph::with_registry(registry());
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.set(total, Sum).link(a, total, Addend).link(b, total, Addend);
    (g, a, b, total)
}

fn value(g: &Graph, e: Entity) -> f64 {
    g.get::<Amount>(e).unwrap().unwrap().0
}

// ---------- the file format ----------

#[test]
fn a_node_file_is_plain_data_and_says_which_rule_backs_what() {
    let (mut g, a, _, total) = sheet();
    g.set(total, Name("Total".into()));
    g.set_runtime(total, "supplier", json!({ "days": 12 }));
    let dir = tempfile::tempdir().unwrap();
    g.save_dir(dir.path()).unwrap();

    let text = fs::read_to_string(dir.path().join(format!("nodes/{total}.yaml"))).unwrap();
    println!("{text}");
    let file: Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(file["id"], json!(total.to_string()));
    assert_eq!(file["components"], json!({ "name": "Total", "supplier": { "days": 12 } }), "plain data, typed or not");
    assert_eq!(file["rules"], json!({ "amount": { "rule": "test/sum" } }));
    assert!(file.get("links").is_none(), "links live with their source");

    let source = fs::read_to_string(dir.path().join(format!("nodes/{a}.yaml"))).unwrap();
    let file: Value = serde_yaml_ng::from_str(&source).unwrap();
    assert_eq!(file["links"], json!([{ "to": total.to_string(), "components": { "addend": null } }]));
    assert_eq!(file["components"]["amount"], json!(1.0));
}

#[test]
fn parameters_are_saved_and_come_back() {
    let (mut g, _, _, total) = sheet();
    g.set(total, Scaled { by: 10.0 });
    assert_eq!(value(&g, total), 30.0);
    let saved = g.save().unwrap();
    let node = saved.nodes.iter().find(|n| n.id == total).unwrap();
    assert_eq!(node.rules["amount"], SavedRule { rule: "test/scaled".into(), params: json!({ "by": 10.0 }) });

    let (loaded, problems) = Graph::load(saved, registry());
    assert_eq!(problems, vec![]);
    assert_eq!(value(&loaded, total), 30.0);
}

#[test]
fn a_loaded_graph_carries_on_like_the_original() {
    let (mut g, a, _, total) = sheet();
    let (mut loaded, _) = Graph::load(g.save().unwrap(), registry());
    assert_eq!(value(&loaded, total), 3.0);
    assert_eq!(loaded.take_ran().len(), 1, "memos aren't saved, so it runs once");
    value(&loaded, total);
    assert!(loaded.take_ran().is_empty(), "then remembers");

    loaded.set(a, Amount(5.0));
    g.set(a, Amount(5.0));
    assert_eq!(value(&loaded, total), value(&g, total));
}

#[test]
fn files_that_did_not_change_are_not_rewritten_and_stale_ones_go() {
    let (g, a, _, _) = sheet();
    let dir = tempfile::tempdir().unwrap();
    g.save_dir(dir.path()).unwrap();
    let path = dir.path().join(format!("nodes/{a}.yaml"));

    // Mark the file so a rewrite would show, even within the clock's resolution.
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    g.save_dir(dir.path()).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before, "a commit only shows real changes");

    let stale = dir.path().join(format!("nodes/{}.yaml", Entity::new()));
    let mine = dir.path().join("nodes/notes.yaml");
    let other = dir.path().join("nodes/README.md");
    fs::write(&stale, "id: gone").unwrap();
    fs::write(&mine, "not a node").unwrap();
    fs::write(&other, "hello").unwrap();
    g.save_dir(dir.path()).unwrap();
    assert!(!stale.exists(), "a file for a node that's gone is removed");
    assert!(mine.exists() && other.exists(), "and anything that isn't a node file is left alone");
}

#[test]
fn a_hand_written_file_works() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("nodes")).unwrap();
    let (a, total) = (Entity::new(), Entity::new());
    fs::write(dir.path().join(format!("nodes/{a}.yaml")), format!("id: {a}\ncomponents:\n  amount: 7.0\nlinks:\n- to: {total}\n  components:\n    addend: null\n")).unwrap();
    fs::write(dir.path().join(format!("nodes/{total}.yaml")), format!("id: {total}\nrules:\n  amount:\n    rule: test/scaled\n    params:\n      by: 2.0\n")).unwrap();

    let (g, problems) = Graph::load_dir(dir.path(), registry()).unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(value(&g, total), 14.0);
}

#[test]
fn an_invalid_file_is_an_error_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("nodes")).unwrap();
    fs::write(dir.path().join("nodes/broken.yaml"), "id: [not, a, uuid]").unwrap();
    let Err(recollect_graph::DirError::Parse(path, _)) = Graph::load_dir(dir.path(), registry()) else { panic!("expected a parse error") };
    assert!(path.ends_with("broken.yaml"), "{path}");
}

// ---------- when a file and the code don't line up ----------

fn node_with_rule(saved: SavedRule, component: &str) -> (recollect_graph::SavedGraph, Entity) {
    let id = Entity::new();
    let node = recollect_graph::SavedNode { id, components: Default::default(), rules: [(component.to_string(), saved)].into_iter().collect(), links: vec![] };
    (recollect_graph::SavedGraph { nodes: vec![node], defaults: Default::default() }, id)
}

#[test]
fn an_unknown_rule_says_so_and_goes_back_out_unchanged() {
    let written = SavedRule { rule: "plugin/missing".into(), params: json!({ "keep": ["me", 1] }) };
    let (saved, id) = node_with_rule(written, "amount");
    let (g, problems) = Graph::load(saved.clone(), registry());
    assert_eq!(problems, vec![LoadProblem::BrokenRule((id, "amount".into()), Error::UnknownRule("plugin/missing".into()))]);
    assert_eq!(g.get::<Amount>(id), Err(Error::UnknownRule("plugin/missing".into())));
    assert_eq!(g.save().unwrap(), saved, "a missing plugin doesn't cost you the data");

    // And it works as soon as something registers it.
    let mut reg = Registry::new();
    reg.runtime_rule(RuntimeRule {
        key: "plugin/missing".into(),
        output: "amount".into(),
        inputs: Inputs::default(),
        compute: Arc::new(|params, _| Ok(Some(json!(params["keep"][1].as_f64().unwrap() + 1.0)))),
    });
    let (g, problems) = Graph::load(saved, Arc::new(reg));
    assert_eq!(problems, vec![]);
    assert_eq!(value(&g, id), 2.0);
}

#[test]
fn a_rule_on_the_wrong_component_says_so() {
    let (saved, id) = node_with_rule(SavedRule { rule: "test/sum".into(), params: Value::Null }, "cost");
    let (g, problems) = Graph::load(saved.clone(), registry());
    let want = Error::WrongRule { rule: "test/sum".into(), component: "cost".into(), provides: "amount".into() };
    assert_eq!(problems, vec![LoadProblem::BrokenRule((id, "cost".into()), want.clone())]);
    assert_eq!(g.get_runtime(id, "cost"), Err(want));
    assert_eq!(g.save().unwrap(), saved);
}

#[test]
fn bad_parameters_are_an_error_in_that_cell() {
    let (saved, id) = node_with_rule(SavedRule { rule: "test/scaled".into(), params: json!({ "by": "lots" }) }, "amount");
    let (g, problems) = Graph::load(saved.clone(), registry());
    assert_eq!(problems.len(), 1);
    assert!(matches!(g.get::<Amount>(id), Err(Error::Malformed { .. })));
    assert_eq!(g.save().unwrap(), saved);
}

#[test]
fn a_cell_reading_a_broken_one_inherits_the_error_and_others_are_fine() {
    let mut g = Graph::with_registry(registry());
    let (broken, reader, fine) = (g.spawn(), g.spawn(), g.spawn());
    g.set_rule(broken, "amount", "plugin/missing", Value::Null);
    g.set(reader, Sum).link(broken, reader, Addend);
    g.set(fine, Amount(1.0));
    assert_eq!(g.get::<Amount>(reader), Err(Error::UnknownRule("plugin/missing".into())));
    assert_eq!(value(&g, fine), 1.0);
}

#[test]
fn a_dangling_link_is_reported_and_kept() {
    let (a, gone) = (Entity::new(), Entity::new());
    let link = recollect_graph::saved::SavedLink { to: gone, components: [("addend".to_string(), Value::Null)].into_iter().collect() };
    let node = recollect_graph::SavedNode { id: a, components: Default::default(), rules: Default::default(), links: vec![link.clone()] };
    let (g, problems) = Graph::load(recollect_graph::SavedGraph { nodes: vec![node], defaults: Default::default() }, registry());
    assert_eq!(problems, vec![LoadProblem::DanglingLink { from: a, to: gone }]);
    let saved = g.save().unwrap();
    assert_eq!(saved.nodes.iter().find(|n| n.id == a).unwrap().links, vec![link]);
}

// ---------- what can't be saved ----------

#[test]
fn a_closure_can_run_but_saving_one_is_an_error_not_a_silent_drop() {
    let (mut g, _, _, total) = sheet();
    g.set(total, rule("throwaway").compute(|_| Ok(Amount(9.0))));
    assert_eq!(value(&g, total), 9.0);
    assert_eq!(g.save(), Err(SaveError::Closures(vec![(total, "amount".into())])));
}

#[test]
fn saving_a_rule_nothing_could_load_is_an_error() {
    let mut g = Graph::new();
    let e = g.spawn();
    g.set(e, Sum);
    assert_eq!(g.get::<Amount>(e), Ok(Some(Amount(0.0))), "it runs without being registered");
    assert_eq!(g.save(), Err(SaveError::Unregistered(vec!["test/sum".into()])), "but the file couldn't be read back");
}

// ---------- behaviour of rules as data ----------

#[test]
fn setting_the_same_rule_and_parameters_changes_nothing() {
    let (mut g, _, _, total) = sheet();
    g.set(total, Scaled { by: 2.0 });
    value(&g, total);
    g.take_ran();

    g.set(total, Scaled { by: 2.0 });
    value(&g, total);
    assert!(g.take_ran().is_empty(), "the same rule with the same parameters");

    g.set(total, Scaled { by: 3.0 });
    assert_eq!(value(&g, total), 9.0);
    assert_eq!(g.take_ran().len(), 1, "different parameters are a different rule");
}

#[test]
fn inputs_can_depend_on_parameters() {
    let (mut g, a, b, total) = sheet();
    g.link(b, total, Bonus);
    g.set(total, SumOver { kind: "bonus".into() });
    assert_eq!(value(&g, total), 2.0, "only the link kind it was told to follow");
    g.set(total, SumOver { kind: "addend".into() });
    assert_eq!(value(&g, total), 3.0);
    assert_eq!(g.rule_of(total, "amount").unwrap().1.incoming, vec!["addend"]);

    let (loaded, _) = Graph::load(g.save().unwrap(), registry());
    assert_eq!(value(&loaded, total), 3.0);
    let _ = a;
}

#[test]
fn a_rule_can_be_attached_by_key_as_a_file_or_the_llm_would() {
    let (mut g, _, _, total) = sheet();
    g.set_rule(total, "amount", "test/scaled", json!({ "by": 2.0 }));
    assert_eq!(value(&g, total), 6.0);
    g.set_rule(total, "amount", "test/nonsense", Value::Null);
    assert_eq!(g.get::<Amount>(total), Err(Error::UnknownRule("test/nonsense".into())), "and a mistake lands in that cell");
}

#[test]
fn the_registry_can_be_listed() {
    let reg = registry();
    let list = reg.list();
    assert_eq!(list.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["test/scaled", "test/sum", "test/sum_over"]);
    let scaled = &list[0];
    assert_eq!(scaled.output, "amount");
    assert_eq!(scaled.inputs["incoming"], vec!["addend"]);
    let schema = scaled.params.as_ref().unwrap();
    assert_eq!(schema["properties"]["by"]["type"], "number", "enough to know how to attach it: {schema}");
    assert_eq!(schema["required"], json!(["by"]));
}

#[test]
fn a_runtime_rule_saves_by_key_like_any_other() {
    let mut reg = Registry::new();
    reg.runtime_rule(RuntimeRule {
        key: "ts/lead".into(),
        output: "lead".into(),
        inputs: Inputs { outgoing: vec!["ships_with".into()], ..Inputs::default() },
        compute: Arc::new(|params, cx| {
            let linked = cx.outgoing_runtime("ships_with")?.len() as f64;
            Ok(Some(json!(params["base"].as_f64().unwrap() + linked)))
        }),
    });
    let reg = Arc::new(reg);
    let mut g = Graph::with_registry(reg.clone());
    let (a, b) = (g.spawn(), g.spawn());
    g.link_runtime(a, b, recollect_graph::Runtime::new("ships_with", json!({})));
    g.set_rule(a, "lead", "ts/lead", json!({ "base": 10.0 }));
    assert_eq!(g.get_runtime(a, "lead").unwrap().unwrap().value, json!(11.0));

    let (loaded, problems) = Graph::load(g.save().unwrap(), reg);
    assert_eq!(problems, vec![]);
    assert_eq!(loaded.get_runtime(a, "lead").unwrap().unwrap().value, json!(11.0));
}

// ---------- defaults ----------

#[test]
fn registry_defaults_come_from_code_and_only_changes_are_saved() {
    #[component]
    struct Doubled(f64);
    #[rule("test/doubled")]
    struct DoubledRule;
    impl Compute for DoubledRule {
        type Output = Option<Doubled>;
        fn compute(&self, cx: &Cx) -> Result<Option<Doubled>, Error> {
            Ok(cx.own::<Amount>()?.map(|a| Doubled(a.0 * 2.0)))
        }
    }
    #[rule("test/tripled")]
    struct TripledRule;
    impl Compute for TripledRule {
        type Output = Option<Doubled>;
        fn compute(&self, cx: &Cx) -> Result<Option<Doubled>, Error> {
            Ok(cx.own::<Amount>()?.map(|a| Doubled(a.0 * 3.0)))
        }
    }
    let mut reg = Registry::new();
    reg.set_default(DoubledRule).rule::<TripledRule>();
    let reg = Arc::new(reg);

    let mut g = Graph::with_registry(reg.clone());
    let (a, note) = (g.spawn(), g.spawn());
    g.set(a, Amount(2.0));
    assert_eq!(g.get::<Doubled>(a).unwrap(), Some(Doubled(4.0)));
    assert_eq!(g.get::<Doubled>(note).unwrap(), None);
    let saved = g.save().unwrap();
    assert!(saved.defaults.rules.is_empty(), "it's the code's default, so a code change takes effect");

    // Overriding it for this graph is saved.
    g.set_default(TripledRule);
    assert_eq!(g.get::<Doubled>(a).unwrap(), Some(Doubled(6.0)));
    let saved = g.save().unwrap();
    assert_eq!(saved.defaults.rules["doubled"].rule, "test/tripled");
    let (loaded, _) = Graph::load(saved, reg);
    assert_eq!(loaded.get::<Doubled>(a).unwrap(), Some(Doubled(6.0)));
}
