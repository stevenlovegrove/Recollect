//! What's written down: the file format, formulas saved as their text, and what happens when a file
//! and the running code don't line up. Nothing should ever be lost.

use std::fs;
use std::sync::Arc;

use recollect_graph::{Cell, CellType, Compile, Computed, Cx, Entity, Error, Graph, LoadProblem, Name, Place, Registry, SavedEdge, SavedGraph, SavedNode, cell};
use serde_json::{Value, json};

#[cell]
struct Amount(f64);

const SHEET: &str = "sheet";
const ADDEND: &str = "addend";
const BONUS: &str = "bonus";

/// The amounts of what links here by an edge with this component, each times `by`.
fn sum_over(cx: &Cx, component: &str, by: f64) -> Cell<Amount> {
    let mut total = 0.0;
    for from in cx.sources(component) {
        total += cx.value::<Amount>(from)?.0;
    }
    Ok(Some(Amount(total * by)))
}

/// Formulas here are `sum`, `scaled <by>` and `sum_over <component>`: about what a rule's key and
/// parameters used to say.
fn compile() -> Compile {
    Arc::new(|text| {
        let words: Vec<&str> = text.split_whitespace().collect();
        match words[..] {
            ["sum"] => Ok(Computed::typed("sum", |cx| sum_over(cx, ADDEND, 1.0))),
            ["scaled", by] => {
                let by: f64 = by.parse().map_err(|_| format!("{by} isn't a number"))?;
                Ok(Computed::typed("scaled", move |cx| sum_over(cx, ADDEND, by)))
            }
            ["sum_over", component] => {
                let component = component.to_string();
                Ok(Computed::typed("sum_over", move |cx| sum_over(cx, &component, 1.0)))
            }
            _ => Err(format!("there's no function called {text}")),
        }
    })
}

fn install(reg: &mut Registry) {
    reg.component(SHEET, "A cell of a sheet: what's added to it, unless something says otherwise.");
    reg.cell::<Amount>(SHEET, "The sum of what's added to it.").builtin(|cx| sum_over(cx, ADDEND, 1.0)).compiles(compile());
    reg.edge(ADDEND, "Added to that.");
    reg.edge(BONUS, "A bonus to that.");
}

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    install(&mut reg);
    Arc::new(reg)
}

/// a + b = total
fn sheet() -> (Graph, Entity, Entity, Entity) {
    let mut g = Graph::with_registry(registry());
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.add(a, SHEET).add(b, SHEET).add(total, SHEET);
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.link(a, total, ADDEND);
    g.link(b, total, ADDEND);
    (g, a, b, total)
}

fn value(g: &Graph, e: Entity) -> f64 {
    g.get::<Amount>(e).unwrap().unwrap().0
}

// ---------- the file format ----------

#[test]
fn a_node_file_is_plain_data_and_says_which_formula_backs_what() {
    let (mut g, a, b, total) = sheet();
    g.set(total, Name("Total".into()));
    g.set_value(total, "supplier", json!({ "days": 12 }));
    g.set_formula(b, "amount", "scaled 2");
    g.set_value(Entity::edge(a, total), "note", json!("by post"));
    let dir = tempfile::tempdir().unwrap();
    g.save_dir(dir.path()).unwrap();

    let text = fs::read_to_string(dir.path().join(format!("nodes/{total}.yaml"))).unwrap();
    println!("{text}");
    let file: Value = serde_yaml_ng::from_str(&text).unwrap();
    assert_eq!(file["id"], json!(total.to_string()));
    assert_eq!(file["components"], json!(["sheet"]));
    assert_eq!(file["cells"], json!({ "name": "Total", "supplier": { "days": 12 } }), "plain data, typed or not");
    assert!(file.get("formulas").is_none(), "what its component works out isn't written down");
    assert!(file.get("links").is_none(), "links live with their source");

    let source = fs::read_to_string(dir.path().join(format!("nodes/{a}.yaml"))).unwrap();
    let file: Value = serde_yaml_ng::from_str(&source).unwrap();
    assert_eq!(file["links"], json!([{ "to": total.to_string(), "components": ["addend"], "cells": { "note": "by post" } }]), "with what the edge says");
    assert_eq!(file["cells"]["amount"], json!(1.0));

    let other = fs::read_to_string(dir.path().join(format!("nodes/{b}.yaml"))).unwrap();
    let file: Value = serde_yaml_ng::from_str(&other).unwrap();
    assert_eq!(file["formulas"], json!({ "amount": "scaled 2" }), "a formula is saved as its text");
}

#[test]
fn a_formula_is_saved_and_comes_back() {
    let (mut g, _, _, total) = sheet();
    g.set_formula(total, "amount", "scaled 10");
    assert_eq!(value(&g, total), 30.0);
    let saved = g.save();
    let node = saved.nodes.iter().find(|n| n.id == total).unwrap();
    assert_eq!(node.formulas["amount"], "scaled 10");

    let (loaded, problems) = Graph::load(saved, registry());
    assert_eq!(problems, vec![]);
    assert_eq!(value(&loaded, total), 30.0);
}

#[test]
fn a_loaded_graph_carries_on_like_the_original() {
    let (mut g, a, _, total) = sheet();
    let (mut loaded, _) = Graph::load(g.save(), registry());
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
    fs::write(dir.path().join(format!("nodes/{a}.yaml")), format!("id: {a}\ncomponents:\n- sheet\ncells:\n  amount: 7.0\nlinks:\n- to: {total}\n  components:\n  - addend\n")).unwrap();
    fs::write(dir.path().join(format!("nodes/{total}.yaml")), format!("id: {total}\ncomponents:\n- sheet\nformulas:\n  amount: scaled 2\n")).unwrap();

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

fn node_with_formula(text: &str, cell: &str, components: &[&str]) -> (SavedGraph, Entity) {
    let id = Entity::new();
    let mut node = SavedNode::empty(id);
    node.components = components.iter().map(|c| c.to_string()).collect();
    node.formulas.insert(cell.to_string(), text.to_string());
    (SavedGraph { nodes: vec![node] }, id)
}

#[test]
fn a_formula_nothing_can_read_says_so_and_goes_back_out_unchanged() {
    let (saved, id) = node_with_formula("plugin_missing", "amount", &[SHEET]);
    let (g, problems) = Graph::load(saved.clone(), registry());
    let want = Error::Failed("there's no function called plugin_missing".into());
    assert_eq!(problems, vec![LoadProblem::BrokenFormula((id, "amount".into()), want.clone())]);
    assert_eq!(g.get::<Amount>(id), Err(want));
    assert_eq!(g.save(), saved, "a missing plugin doesn't cost you the data");

    // And it works as soon as something can read it.
    let mut reg = Registry::new();
    reg.component(SHEET, "");
    reg.cell::<Amount>(SHEET, "").compiles(Arc::new(|text| match text {
        "plugin_missing" => Ok(Computed::typed("plugin_missing", |_| Ok(Some(Amount(2.0))))),
        _ => Err(format!("there's no function called {text}")),
    }));
    let (g, problems) = Graph::load(saved, Arc::new(reg));
    assert_eq!(problems, vec![]);
    assert_eq!(value(&g, id), 2.0);
}

#[test]
fn a_formula_on_a_cell_that_cant_hold_one_says_so() {
    // Nothing here reads formulas on the workspace's own cells.
    let (saved, id) = node_with_formula("sum", "cost", &[]);
    let (g, problems) = Graph::load(saved.clone(), registry());
    let want = Error::Failed("nothing here can read a formula".into());
    assert_eq!(problems, vec![LoadProblem::BrokenFormula((id, "cost".into()), want.clone())]);
    assert_eq!(g.get_value(id, "cost"), Err(want));
    assert_eq!(g.save(), saved);
}

#[test]
fn a_formula_that_doesnt_parse_is_an_error_in_that_cell() {
    let (saved, id) = node_with_formula("scaled lots", "amount", &[SHEET]);
    let (g, problems) = Graph::load(saved.clone(), registry());
    assert_eq!(problems.len(), 1);
    assert_eq!(g.get::<Amount>(id), Err(Error::Failed("lots isn't a number".into())));
    assert_eq!(g.save(), saved);
}

#[test]
fn a_cell_reading_a_broken_one_inherits_the_error_and_others_are_fine() {
    let mut g = Graph::with_registry(registry());
    let (broken, reader, fine) = (g.spawn(), g.spawn(), g.spawn());
    g.add(broken, SHEET).add(reader, SHEET).add(fine, SHEET);
    g.set_formula(broken, "amount", "plugin_missing");
    g.link(broken, reader, ADDEND);
    g.set(fine, Amount(1.0));
    assert_eq!(g.get::<Amount>(reader), Err(Error::Failed("there's no function called plugin_missing".into())));
    assert_eq!(value(&g, fine), 1.0);
}

#[test]
fn a_dangling_link_is_reported_and_kept() {
    let (a, gone) = (Entity::new(), Entity::new());
    let link = SavedEdge { to: gone, components: vec![ADDEND.to_string()], cells: [("note".to_string(), json!(1))].into_iter().collect(), formulas: Default::default() };
    let mut node = SavedNode::empty(a);
    node.links = vec![link.clone()];
    let (g, problems) = Graph::load(SavedGraph { nodes: vec![node] }, registry());
    assert_eq!(problems, vec![LoadProblem::DanglingLink { from: a, to: gone }]);
    let saved = g.save();
    assert_eq!(saved.nodes.iter().find(|n| n.id == a).unwrap().links, vec![link]);
}

// ---------- behaviour of formulas as data ----------

#[test]
fn setting_the_same_formula_changes_nothing() {
    let (mut g, _, _, total) = sheet();
    g.set_formula(total, "amount", "scaled 2");
    value(&g, total);
    g.take_ran();

    g.set_formula(total, "amount", "scaled 2");
    value(&g, total);
    assert!(g.take_ran().is_empty(), "the same formula");

    g.set_formula(total, "amount", "scaled 3");
    assert_eq!(value(&g, total), 9.0);
    assert_eq!(g.take_ran().len(), 1, "different text is a different formula");
}

#[test]
fn what_a_formula_follows_can_depend_on_its_text() {
    let (mut g, _, b, total) = sheet();
    g.link(b, total, BONUS);
    g.set_formula(total, "amount", "sum_over bonus");
    assert_eq!(value(&g, total), 2.0, "only the edges it was told to follow");
    g.set_formula(total, "amount", "sum_over addend");
    assert_eq!(value(&g, total), 3.0);

    let (loaded, _) = Graph::load(g.save(), registry());
    assert_eq!(value(&loaded, total), 3.0);
}

#[test]
fn a_formula_can_be_written_as_a_file_or_the_llm_would() {
    let (mut g, _, _, total) = sheet();
    g.set_formula(total, "amount", "scaled 2");
    assert_eq!(value(&g, total), 6.0);
    g.set_formula(total, "amount", "nonsense");
    assert_eq!(g.get::<Amount>(total), Err(Error::Failed("there's no function called nonsense".into())), "and a mistake lands in that cell");
    g.clear(total, "amount");
    assert_eq!(value(&g, total), 3.0, "and clearing it brings back what its component works out");
}

#[test]
fn the_registry_can_be_listed() {
    let reg = registry();
    let list: Vec<(&str, Place)> = reg.components().map(|c| (c.name.as_str(), c.place)).collect();
    assert_eq!(list, vec![(ADDEND, Place::Edge), (BONUS, Place::Edge), (SHEET, Place::Node)]);
    let sheet = reg.component_def(SHEET).unwrap();
    assert_eq!(sheet.cells, vec![Amount::NAME]);
    let amount = reg.cell_def(Amount::NAME).unwrap();
    assert_eq!(amount.component, SHEET);
    assert!(amount.computed() && amount.formulas() && amount.settable(), "enough to know what can be written on it");
    assert_eq!(reg.owner("supplier"), None, "and a cell no component brings is the workspace's own");
}

#[test]
fn a_formula_with_no_rust_type_saves_like_any_other() {
    let mut reg = Registry::new();
    reg.free_formulas(Arc::new(|text| {
        let base: f64 = text.strip_prefix("lead ").and_then(|b| b.parse().ok()).ok_or_else(|| format!("can't read {text}"))?;
        Ok(Computed::json("lead", move |cx| Ok(Some(json!(base + cx.outgoing("ships_with").len() as f64)))))
    }));
    let reg = Arc::new(reg);
    let mut g = Graph::with_registry(reg.clone());
    let (a, b) = (g.spawn(), g.spawn());
    g.link(a, b, "ships_with");
    g.set_formula(a, "lead", "lead 10");
    assert_eq!(g.get_value(a, "lead").unwrap(), Some(json!(11.0)));

    let (loaded, problems) = Graph::load(g.save(), reg);
    assert_eq!(problems, vec![]);
    assert_eq!(loaded.get_value(a, "lead").unwrap(), Some(json!(11.0)));
}

// ---------- defaults ----------

#[test]
fn defaults_come_from_code_and_only_what_is_written_is_saved() {
    #[cell]
    struct Doubled(f64);
    let mut reg = Registry::new();
    install(&mut reg);
    reg.component("doubling", "Twice its amount.").requires(SHEET);
    reg.cell::<Doubled>("doubling", "Twice its amount.").builtin(|cx| Ok(cx.own::<Amount>()?.map(|a| Doubled(a.0 * 2.0))));
    let reg = Arc::new(reg);

    let mut g = Graph::with_registry(reg.clone());
    let (a, note) = (g.spawn(), g.spawn());
    g.add(a, "doubling").set(a, Amount(2.0));
    assert_eq!(g.get::<Doubled>(a).unwrap(), Some(Doubled(4.0)));
    assert_eq!(g.get::<Doubled>(note).unwrap(), None, "a node without the component hasn't the cell");
    let saved = g.save();
    let mine = saved.nodes.iter().find(|n| n.id == a).unwrap();
    assert!(!mine.cells.contains_key("doubled") && mine.formulas.is_empty(), "it's the code's default, so a code change takes effect");
    assert_eq!(mine.components, vec!["doubling"], "and what it requires comes with it, unwritten");

    // Writing over it on a node is saved.
    g.set(a, Doubled(6.0));
    let saved = g.save();
    assert_eq!(saved.nodes.iter().find(|n| n.id == a).unwrap().cells["doubled"], json!(6.0));
    let (loaded, _) = Graph::load(saved, reg);
    assert_eq!(loaded.get::<Doubled>(a).unwrap(), Some(Doubled(6.0)));
}
