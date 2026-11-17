//! `Space::distinct` builds "no two the same" in one pass. It has to be exactly every pair differing,
//! whatever the items are: several on one choice, some the same in every world, values repeated across
//! options, choices of different sizes.

use recollect_graph::worlds::{Cond, Space};

/// A small, fixed sequence of numbers, so a failure can be run again.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

#[test]
fn no_two_the_same_in_one_pass_is_every_pair_differing() {
    let mut r = Lcg(7);
    for round in 0..300 {
        let space = Space::new();
        let vars: Vec<_> = (0..1 + r.below(4))
            .map(|i| {
                let options: Vec<String> = (0..1 + r.below(4)).map(|o| format!("o{o}")).collect();
                let options: Vec<&str> = options.iter().map(String::as_str).collect();
                space.uniform(&format!("v{i}"), &options)
            })
            .collect();
        let items: Vec<Cond<u64>> = (0..1 + r.below(5))
            .map(|_| {
                if r.below(4) == 0 {
                    space.pure(r.below(5))
                } else {
                    let var = &vars[r.below(vars.len() as u64) as usize];
                    let to: Vec<u64> = var.options().iter().map(|_| r.below(5)).collect();
                    let options = var.options();
                    var.value().map(|o| to[options.iter().position(|x| x == o).unwrap()])
                }
            })
            .collect();
        let mut pairwise = space.always();
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                pairwise = &pairwise & &a.zip(b, |x, y| x != y).holds(|d| *d);
            }
        }
        let fast = space.distinct(&items.iter().collect::<Vec<_>>()).expect("each item is on one choice at most");
        assert_eq!(fast, pairwise, "round {round}: {} items over {} choices", items.len(), vars.len());
    }
}

#[test]
fn an_item_on_two_choices_is_left_to_the_pairs() {
    let space = Space::new();
    let (a, b) = (space.uniform("a", &["0", "1"]), space.uniform("b", &["0", "1"]));
    let sum = a.value().zip(&b.value(), |x, y| format!("{x}{y}"));
    assert!(space.distinct(&[&sum, &a.value()]).is_none());
}

#[test]
fn all_different_over_nine_choices_of_nine_is_small() {
    let space = Space::new();
    let digits: Vec<String> = (1..=9).map(|d| d.to_string()).collect();
    let digits: Vec<&str> = digits.iter().map(String::as_str).collect();
    let cells: Vec<Cond<String>> = (0..9).map(|i| space.uniform(&format!("c{i}"), &digits).value()).collect();
    let all = space.distinct(&cells.iter().collect::<Vec<_>>()).unwrap();
    let vars: Vec<_> = cells.iter().flat_map(|c| c.vars()).collect();
    assert_eq!(all.count(&vars), 362_880, "9!");
    // One node for each set of digits used so far, short of all nine: 2^9 - 1.
    assert_eq!(all.size(), 511);
}
