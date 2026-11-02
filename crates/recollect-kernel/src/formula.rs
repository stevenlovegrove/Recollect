//! Exact boolean functions over the open decisions, as a reduced, hash-consed multi-valued
//! decision diagram. Each open decision is a variable whose values are its live options, and
//! a "world" is one assignment of every variable. Equal functions get equal handles, so
//! "true in every world" is just `f == TRUE`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

pub type F = u32;
pub const FALSE: F = 0;
pub const TRUE: F = 1;
const LEAF: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct Var {
    pub name: String,
    pub options: Vec<String>,
    /// Normalised to sum to 1. Only the optional probability layer reads these.
    pub weights: Vec<f64>,
    /// Which options are gaps rather than named answers.
    pub unknown: Vec<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct Mdd {
    pub vars: Vec<Var>,
    nodes: Vec<(u32, Vec<F>)>,
    unique: HashMap<(u32, Vec<F>), F>,
    and_memo: HashMap<(F, F), F>,
    or_memo: HashMap<(F, F), F>,
    not_memo: HashMap<F, F>,
}

impl Mdd {
    pub fn new(vars: Vec<Var>) -> Self {
        let mut m = Mdd { vars, ..Default::default() };
        m.nodes.push((LEAF, vec![]));
        m.nodes.push((LEAF, vec![]));
        m
    }

    fn mk(&mut self, var: u32, kids: Vec<F>) -> F {
        if kids.iter().all(|k| *k == kids[0]) {
            return kids[0];
        }
        if let Some(&f) = self.unique.get(&(var, kids.clone())) {
            return f;
        }
        let f = self.nodes.len() as F;
        self.nodes.push((var, kids.clone()));
        self.unique.insert((var, kids), f);
        f
    }

    /// True exactly in the worlds where decision `var` takes option `opt`.
    pub fn literal(&mut self, var: usize, opt: usize) -> F {
        let kids = (0..self.vars[var].options.len()).map(|i| if i == opt { TRUE } else { FALSE }).collect();
        self.mk(var as u32, kids)
    }

    fn var_of(&self, f: F) -> u32 {
        self.nodes[f as usize].0
    }

    fn cofactor(&self, f: F, var: u32, i: usize) -> F {
        let (v, kids) = &self.nodes[f as usize];
        if *v == var { kids[i] } else { f }
    }

    pub fn and(&mut self, a: F, b: F) -> F {
        if a == FALSE || b == FALSE {
            return FALSE;
        }
        if a == TRUE || a == b {
            return b;
        }
        if b == TRUE {
            return a;
        }
        let key = (a.min(b), a.max(b));
        if let Some(&f) = self.and_memo.get(&key) {
            return f;
        }
        let var = self.var_of(a).min(self.var_of(b));
        let n = self.vars[var as usize].options.len();
        let kids = (0..n)
            .map(|i| {
                let (x, y) = (self.cofactor(a, var, i), self.cofactor(b, var, i));
                self.and(x, y)
            })
            .collect();
        let f = self.mk(var, kids);
        self.and_memo.insert(key, f);
        f
    }

    pub fn or(&mut self, a: F, b: F) -> F {
        if a == TRUE || b == TRUE {
            return TRUE;
        }
        if a == FALSE || a == b {
            return b;
        }
        if b == FALSE {
            return a;
        }
        let key = (a.min(b), a.max(b));
        if let Some(&f) = self.or_memo.get(&key) {
            return f;
        }
        let var = self.var_of(a).min(self.var_of(b));
        let n = self.vars[var as usize].options.len();
        let kids = (0..n)
            .map(|i| {
                let (x, y) = (self.cofactor(a, var, i), self.cofactor(b, var, i));
                self.or(x, y)
            })
            .collect();
        let f = self.mk(var, kids);
        self.or_memo.insert(key, f);
        f
    }

    pub fn not(&mut self, a: F) -> F {
        match a {
            FALSE => TRUE,
            TRUE => FALSE,
            _ => {
                if let Some(&f) = self.not_memo.get(&a) {
                    return f;
                }
                let (var, kids) = self.nodes[a as usize].clone();
                let kids = kids.into_iter().map(|k| self.not(k)).collect();
                let f = self.mk(var, kids);
                self.not_memo.insert(a, f);
                f
            }
        }
    }

    /// Probability of `f` under the option weights, treating decisions as independent of each
    /// other. That independence is between decisions only; correlated paths through the same
    /// decision are exact, because each variable appears once.
    pub fn prob(&self, f: F) -> f64 {
        fn go(m: &Mdd, f: F, memo: &mut HashMap<F, f64>) -> f64 {
            match f {
                FALSE => 0.0,
                TRUE => 1.0,
                _ => {
                    if let Some(&p) = memo.get(&f) {
                        return p;
                    }
                    let (var, kids) = &m.nodes[f as usize];
                    let w = &m.vars[*var as usize].weights;
                    let p = kids.iter().zip(w).map(|(k, w)| w * go(m, *k, memo)).sum();
                    memo.insert(f, p);
                    p
                }
            }
        }
        go(self, f, &mut HashMap::new())
    }

    /// True in every world where each decision lands on a named option.
    pub fn holds_among_known(&self, f: F) -> bool {
        fn go(m: &Mdd, f: F, seen: &mut BTreeSet<F>) -> bool {
            match f {
                FALSE => false,
                TRUE => true,
                _ if !seen.insert(f) => true,
                _ => {
                    let (var, kids) = &m.nodes[f as usize];
                    kids.iter().zip(&m.vars[*var as usize].unknown).all(|(k, gap)| *gap || go(m, *k, seen))
                }
            }
        }
        go(self, f, &mut BTreeSet::new())
    }

    /// The decisions `f` actually depends on.
    pub fn support(&self, f: F) -> BTreeSet<String> {
        fn go(m: &Mdd, f: F, seen: &mut BTreeSet<F>, out: &mut BTreeSet<String>) {
            if f <= TRUE || !seen.insert(f) {
                return;
            }
            let (var, kids) = &m.nodes[f as usize];
            out.insert(m.vars[*var as usize].name.clone());
            for k in kids {
                go(m, *k, seen, out);
            }
        }
        let mut out = BTreeSet::new();
        go(self, f, &mut BTreeSet::new(), &mut out);
        out
    }

    /// Evaluate in one world: `decision → option`. Decisions not listed must not matter.
    pub fn eval(&self, mut f: F, world: &BTreeMap<&str, &str>) -> bool {
        while f > TRUE {
            let (var, kids) = &self.nodes[f as usize];
            let v = &self.vars[*var as usize];
            let chosen = world.get(v.name.as_str()).unwrap_or_else(|| panic!("world doesn't set {}", v.name));
            let i = v.options.iter().position(|o| o == chosen).unwrap_or_else(|| panic!("{} has no option {chosen}", v.name));
            f = kids[i];
        }
        f == TRUE
    }

    /// The function as readable logic, e.g. `calib∈{cad,free}`. Canonical: equal functions
    /// print the same, and it is exact, not a summary.
    pub fn show(&self, f: F) -> String {
        match f {
            FALSE => "never".into(),
            TRUE => "always".into(),
            _ => self.terms(f).join(" ∨ "),
        }
    }

    fn terms(&self, f: F) -> Vec<String> {
        if f == TRUE {
            return vec![String::new()];
        }
        if f == FALSE {
            return vec![];
        }
        let (var, kids) = &self.nodes[f as usize];
        let v = &self.vars[*var as usize];
        let mut groups: Vec<(F, Vec<&str>)> = Vec::new();
        for (kid, opt) in kids.iter().zip(&v.options) {
            match groups.iter_mut().find(|(k, _)| k == kid) {
                Some((_, opts)) => opts.push(opt),
                None => groups.push((*kid, vec![opt])),
            }
        }
        let mut out = Vec::new();
        for (kid, opts) in groups {
            let lit = if opts.len() == 1 { format!("{}={}", v.name, opts[0]) } else { format!("{}∈{{{}}}", v.name, opts.join(",")) };
            for t in self.terms(kid) {
                out.push(if t.is_empty() { lit.clone() } else { format!("{lit} ∧ {t}") });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, opts: &[&str]) -> Var {
        Var { name: name.into(), options: opts.iter().map(|s| s.to_string()).collect(), weights: vec![1.0 / opts.len() as f64; opts.len()], unknown: vec![false; opts.len()] }
    }

    #[test]
    fn canonical_and_exact() {
        let mut m = Mdd::new(vec![var("d", &["a", "b", "c"])]);
        let (a, b, c) = (m.literal(0, 0), m.literal(0, 1), m.literal(0, 2));
        assert_eq!(m.and(a, b), FALSE, "options of one decision exclude each other");
        let ab = m.or(a, b);
        let abc = m.or(ab, c);
        assert_eq!(abc, TRUE, "options are exhaustive");
        assert_eq!(m.not(c), ab, "equal functions share a handle");
        assert_eq!(m.show(ab), "d∈{a,b}");
        assert!((m.prob(ab) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn correlated_paths_are_not_treated_as_independent() {
        // (root=a ∧ x=p) ∨ root=b: the independence assumption would give 62.5%.
        let mut m = Mdd::new(vec![var("root", &["a", "b"]), var("x", &["p", "q"])]);
        let (ra, rb, xp) = (m.literal(0, 0), m.literal(0, 1), m.literal(1, 0));
        let both = m.and(ra, xp);
        let f = m.or(both, rb);
        assert!((m.prob(f) - 0.75).abs() < 1e-9);
        assert_eq!(m.show(f), "root=a ∧ x=p ∨ root=b");
        assert!(m.eval(f, &BTreeMap::from([("root", "b"), ("x", "q")])));
    }
}
