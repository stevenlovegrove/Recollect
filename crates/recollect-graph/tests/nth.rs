//! Picking one world by where it comes, rather than listing every world before it.

use recollect_graph::worlds::Space;

#[test]
fn the_kth_world_is_the_kth_listed_and_there_are_as_many_as_counted() {
    let space = Space::new();
    let opts = |names: &[&str]| names.iter().map(|n| (n.to_string(), 1.0, false)).collect::<Vec<_>>();
    let a = space.var("a", &opts(&["x", "y", "z"]));
    let b = space.var("b", &opts(&["p", "q"]));
    let c = space.var("c", &opts(&["r", "s", "t"]));
    // Some worlds ruled out, so the counts under each option differ.
    let not_xq = !&(&a.is("x") & &b.is("q"));
    let not_s = !&c.is("s");
    let left = &not_xq & &not_s;
    let vars = [a, b, c];
    let n = left.count(&vars);
    let listed = left.solutions(&vars, usize::MAX);
    assert_eq!(listed.len() as u128, n);
    for (k, world) in listed.iter().enumerate() {
        assert_eq!(left.nth(&vars, k as u128).as_ref(), Some(world), "world {k}");
    }
    assert_eq!(left.nth(&vars, n), None, "and there's no more after the last");
}

#[test]
fn far_along_a_space_too_big_to_list_is_still_one_step() {
    let space = Space::new();
    let digits: Vec<(String, f64, bool)> = (0..10).map(|d| (d.to_string(), 1.0, false)).collect();
    let vars: Vec<_> = (0..30).map(|i| space.var(&format!("d{i}"), &digits)).collect();
    let all = space.always();
    let n = all.count(&vars);
    assert_eq!(n, 10u128.pow(30));
    // The last world is every digit 9, and the one before it ends in 8.
    let last = all.nth(&vars, n - 1).unwrap();
    assert!(last.values().all(|d| d == "9"));
    let before = all.nth(&vars, n - 2).unwrap();
    assert_eq!(before.values().filter(|d| *d == "8").count(), 1);
}
