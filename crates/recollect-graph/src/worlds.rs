//! Worlds, as ordinary value types. The core doesn't know about any of this: `Bool` and
//! `Cond<T>` are just values a cell can hold.
//!
//! A variable is one open choice. A world assigns each variable one option. `Bool` is the exact
//! set of worlds where something holds, as a reduced, hash-consed multi-valued decision diagram,
//! so equal functions compare equal in constant time and a memoised cell stops propagating when
//! its function hasn't changed. `Cond<T>` is any value that differs between worlds.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use serde::de::Error as _;
use serde_json::Value;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

type F = u32;
/// The most ways of holding a function is written out as. Past this, it says how many more there are:
/// writing them all can take longer than working them out, and nobody reads that many.
const MOST_TERMS: usize = 24;
const FALSE: F = 0;
const TRUE: F = 1;
const LEAF: u32 = u32::MAX;

#[derive(Debug)]
struct VarDef {
    name: String,
    options: Vec<String>,
    weights: Vec<f64>,
    gaps: Vec<bool>,
    /// For one of several variables that are one draw, each for a different set of outcomes: what the
    /// draw is called, and the worlds where this one is the one in use. See `Var::in_group`.
    group: Option<(String, F)>,
}

#[derive(Debug, Default)]
struct Mdd {
    vars: Vec<VarDef>,
    index: HashMap<(String, Vec<String>, Vec<u64>), u32>,
    nodes: Vec<(u32, Vec<F>)>,
    unique: HashMap<(u32, Vec<F>), F>,
    and_memo: HashMap<(F, F), F>,
    or_memo: HashMap<(F, F), F>,
    not_memo: HashMap<F, F>,
}

impl Mdd {
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

    fn var_of(&self, f: F) -> u32 {
        self.nodes[f as usize].0
    }

    fn cofactor(&self, f: F, var: u32, i: usize) -> F {
        let (v, kids) = &self.nodes[f as usize];
        if *v == var { kids[i] } else { f }
    }

    fn apply(&mut self, a: F, b: F, is_and: bool) -> F {
        let (zero, one) = if is_and { (FALSE, TRUE) } else { (TRUE, FALSE) };
        if a == zero || b == zero {
            return zero;
        }
        if a == one || a == b {
            return b;
        }
        if b == one {
            return a;
        }
        let key = (a.min(b), a.max(b));
        let known = if is_and { self.and_memo.get(&key) } else { self.or_memo.get(&key) };
        if let Some(&f) = known {
            return f;
        }
        let var = self.var_of(a).min(self.var_of(b));
        let n = self.vars[var as usize].options.len();
        let kids = (0..n)
            .map(|i| {
                let (x, y) = (self.cofactor(a, var, i), self.cofactor(b, var, i));
                self.apply(x, y, is_and)
            })
            .collect();
        let f = self.mk(var, kids);
        if is_and { self.and_memo.insert(key, f) } else { self.or_memo.insert(key, f) };
        f
    }

    fn not(&mut self, a: F) -> F {
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

    /// `f` with these variables fixed.
    fn given(&mut self, f: F, picks: &HashMap<u32, usize>, memo: &mut HashMap<F, F>) -> F {
        if f <= TRUE {
            return f;
        }
        if let Some(&known) = memo.get(&f) {
            return known;
        }
        let (var, kids) = self.nodes[f as usize].clone();
        let out = match picks.get(&var) {
            Some(&i) => self.given(kids[i], picks, memo),
            None => {
                let kids = kids.into_iter().map(|k| self.given(k, picks, memo)).collect();
                self.mk(var, kids)
            }
        };
        memo.insert(f, out);
        out
    }

    fn support(&self, roots: impl IntoIterator<Item = F>) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack: Vec<F> = roots.into_iter().collect();
        while let Some(f) = stack.pop() {
            if f <= TRUE || !seen.insert(f) {
                continue;
            }
            let (var, kids) = &self.nodes[f as usize];
            out.insert(*var);
            stack.extend(kids);
        }
        out
    }

    fn prob(&self, f: F, memo: &mut HashMap<F, f64>) -> f64 {
        match f {
            FALSE => 0.0,
            TRUE => 1.0,
            _ => {
                if let Some(&p) = memo.get(&f) {
                    return p;
                }
                let (var, kids) = &self.nodes[f as usize];
                let w = &self.vars[*var as usize].weights;
                let p = kids.iter().zip(w).map(|(k, w)| w * self.prob(*k, memo)).sum();
                memo.insert(f, p);
                p
            }
        }
    }

    /// How likely `f` is, over the variables that are matters of chance, for each way the rest could go:
    /// mutually exclusive guards over the rest, each with a chance. `None` if that's more than `most`
    /// different chances.
    fn weigh(&mut self, f: F, chance: &[bool], memo: &mut HashMap<F, Vec<(F, f64)>>, most: usize) -> Option<Vec<(F, f64)>> {
        if f <= TRUE {
            return Some(vec![(TRUE, f64::from(f))]);
        }
        if let Some(known) = memo.get(&f) {
            return Some(known.clone());
        }
        // Chances that came about by different sums are the same chance.
        fn put(m: &mut Mdd, out: &mut Vec<(F, f64)>, g: F, v: f64) {
            if g == FALSE {
                return;
            }
            match out.iter_mut().find(|(_, x)| (*x - v).abs() < 1e-12) {
                Some((existing, _)) => *existing = m.apply(*existing, g, false),
                None => out.push((g, v)),
            }
        }
        let (var, kids) = self.nodes[f as usize].clone();
        let mut out: Vec<(F, f64)> = Vec::new();
        if chance[var as usize] {
            // Each option as likely as it is, whatever it leads to.
            let weights = self.vars[var as usize].weights.clone();
            out.push((TRUE, 0.0));
            for (kid, w) in kids.into_iter().zip(weights) {
                let each = self.weigh(kid, chance, memo, most)?;
                let mut next = Vec::new();
                for (g, so_far) in &out {
                    for (h, v) in &each {
                        let both = self.apply(*g, *h, true);
                        put(self, &mut next, both, so_far + w * v);
                    }
                }
                if next.len() > most {
                    return None;
                }
                out = next;
            }
        } else {
            let n = kids.len();
            for (i, kid) in kids.into_iter().enumerate() {
                let each = self.weigh(kid, chance, memo, most)?;
                let here = self.mk(var, (0..n).map(|k| if k == i { TRUE } else { FALSE }).collect());
                for (g, v) in each {
                    let both = self.apply(here, g, true);
                    put(self, &mut out, both, v);
                }
            }
            if out.len() > most {
                return None;
            }
        }
        memo.insert(f, out.clone());
        Some(out)
    }

    /// `f` where some option of each of these variables makes it hold: they're no longer in it.
    fn exists(&mut self, f: F, vars: &BTreeSet<u32>, memo: &mut HashMap<F, F>) -> F {
        if f <= TRUE {
            return f;
        }
        if let Some(&known) = memo.get(&f) {
            return known;
        }
        let (var, kids) = self.nodes[f as usize].clone();
        let kids: Vec<F> = kids.into_iter().map(|k| self.exists(k, vars, memo)).collect();
        let out = if vars.contains(&var) { kids.into_iter().fold(FALSE, |acc, k| self.apply(acc, k, false)) } else { self.mk(var, kids) };
        memo.insert(f, out);
        out
    }

    /// The ways a function holds, as "and"s of choices, up to `most` of them. There can be very many,
    /// so they're listed only as far as asked.
    fn terms(&self, f: F, label: &dyn Fn(&str) -> String, most: usize) -> Vec<String> {
        let mut out = Vec::new();
        self.walk(f, String::new(), label, most, &mut out);
        out
    }

    fn walk(&self, f: F, prefix: String, label: &dyn Fn(&str) -> String, most: usize, out: &mut Vec<String>) {
        if out.len() >= most || f == FALSE {
            return;
        }
        if f == TRUE {
            out.push(prefix);
            return;
        }
        let (var, kids) = &self.nodes[f as usize];
        let v = &self.vars[*var as usize];
        for (kid, opts) in Self::groups(kids, &v.options) {
            let name = label(&v.name);
            let opts: Vec<String> = opts.iter().map(|o| label(o)).collect();
            let lit = if opts.len() == 1 { format!("{name}={}", opts[0]) } else { format!("{name}∈{{{}}}", opts.join(",")) };
            self.walk(kid, if prefix.is_empty() { lit } else { format!("{prefix} ∧ {lit}") }, label, most, out);
        }
    }

    /// A node's children, with the options leading to each.
    fn groups<'o>(kids: &[F], options: &'o [String]) -> Vec<(F, Vec<&'o str>)> {
        let mut groups: Vec<(F, Vec<&str>)> = Vec::new();
        for (kid, opt) in kids.iter().zip(options) {
            match groups.iter_mut().find(|(k, _)| k == kid) {
                Some((_, opts)) => opts.push(opt),
                None => groups.push((*kid, vec![opt])),
            }
        }
        groups
    }

    /// How many ways `terms` would list, without listing them.
    fn count_terms(&self, f: F, memo: &mut HashMap<F, u128>) -> u128 {
        if f == FALSE {
            return 0;
        }
        if f == TRUE {
            return 1;
        }
        if let Some(n) = memo.get(&f) {
            return *n;
        }
        let (var, kids) = &self.nodes[f as usize];
        let n = Self::groups(kids, &self.vars[*var as usize].options).into_iter().fold(0u128, |t, (kid, _)| t.saturating_add(self.count_terms(kid, memo)));
        memo.insert(f, n);
        n
    }
}

/// The variables and the diagram values share. Variables are identified by their name, options
/// and weights, so asking again gets the same one, and handles stay valid across edits to the
/// graph. New variables go last in the order. Values normally live in the global one, which is
/// what lets one be read back from JSON with no context.
#[derive(Clone)]
pub struct Space(Arc<Mutex<Mdd>>);

static GLOBAL: LazyLock<Space> = LazyLock::new(Space::new);

impl fmt::Debug for Space {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Space({} vars)", self.lock().vars.len())
    }
}

impl Default for Space {
    fn default() -> Self {
        Self::new()
    }
}

impl Space {
    pub fn global() -> Self {
        GLOBAL.clone()
    }

    /// A separate space. Values from different spaces can't be combined or compared.
    pub fn new() -> Self {
        let mut m = Mdd::default();
        m.nodes.extend([(LEAF, vec![]), (LEAF, vec![])]);
        Space(Arc::new(Mutex::new(m)))
    }

    fn lock(&self) -> MutexGuard<'_, Mdd> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One open choice. `options` are (name, weight, is a gap).
    pub fn var(&self, name: &str, options: &[(String, f64, bool)]) -> Var {
        let total: f64 = options.iter().map(|o| o.1).sum();
        let weights: Vec<f64> = options.iter().map(|o| if total > 0.0 { o.1 / total } else { 1.0 / options.len() as f64 }).collect();
        let names: Vec<String> = options.iter().map(|o| o.0.clone()).collect();
        let gaps = options.iter().map(|o| o.2).collect();
        self.var_exact(name, names, weights, gaps)
    }

    /// As `var`, with weights that are already normalised.
    fn var_exact(&self, name: &str, names: Vec<String>, weights: Vec<f64>, gaps: Vec<bool>) -> Var {
        let key = (name.to_string(), names.clone(), weights.iter().map(|w| w.to_bits()).collect());
        let mut m = self.lock();
        let idx = match m.index.get(&key) {
            Some(&i) => i,
            None => {
                let i = m.vars.len() as u32;
                m.vars.push(VarDef { name: name.to_string(), options: names, weights, gaps, group: None });
                m.index.insert(key, i);
                i
            }
        };
        Var { space: self.clone(), idx }
    }

    /// One open choice among equally likely options.
    pub fn uniform(&self, name: &str, options: &[&str]) -> Var {
        let opts: Vec<(String, f64, bool)> = options.iter().map(|o| (o.to_string(), 1.0, false)).collect();
        self.var(name, &opts)
    }

    pub fn truth(&self, v: bool) -> Bool {
        Bool { space: self.clone(), f: if v { TRUE } else { FALSE } }
    }

    pub fn always(&self) -> Bool {
        self.truth(true)
    }

    pub fn never(&self) -> Bool {
        self.truth(false)
    }

    pub fn pure<T>(&self, v: T) -> Cond<T> {
        Cond { space: self.clone(), cases: vec![(TRUE, v)] }
    }

    pub fn all(&self, items: impl IntoIterator<Item = Bool>) -> Bool {
        items.into_iter().fold(self.always(), |a, b| &a & &b)
    }

    pub fn any(&self, items: impl IntoIterator<Item = Bool>) -> Bool {
        items.into_iter().fold(self.never(), |a, b| &a | &b)
    }

    /// Some choices pinned to an option each. A choice this space doesn't know, or an option it
    /// doesn't have, pins nothing.
    pub fn picks(&self, world: &BTreeMap<String, String>) -> Picks {
        let m = self.lock();
        let mut picks = HashMap::new();
        for (i, v) in m.vars.iter().enumerate() {
            if let Some(chosen) = world.get(&v.name)
                && let Some(at) = v.options.iter().position(|o| o == chosen)
            {
                picks.insert(i as u32, at);
            }
        }
        Picks(picks)
    }

    /// The variables that are one draw, by what the draw is called. See `Var::in_group`.
    pub fn draw_group(&self, name: &str) -> Option<DrawGroup> {
        let vars: Vec<Var> = {
            let m = self.lock();
            m.vars.iter().enumerate().filter(|(_, v)| v.group.as_ref().is_some_and(|(g, _)| g == name)).map(|(i, _)| Var { space: self.clone(), idx: i as u32 }).collect()
        };
        DrawGroup::gather(vars).pop()
    }

    /// How many diagram nodes exist: a measure of how hard things have been.
    pub fn size(&self) -> usize {
        self.lock().nodes.len()
    }

    fn and(&self, a: F, b: F) -> F {
        self.lock().apply(a, b, true)
    }

    fn or(&self, a: F, b: F) -> F {
        self.lock().apply(a, b, false)
    }

    fn not(&self, a: F) -> F {
        self.lock().not(a)
    }
}

/// Some choices pinned to an option each, from `Space::picks`. Everything narrowed by these is
/// still exact: what's left is the set of worlds consistent with them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picks(HashMap<u32, usize>);

/// One open choice.
#[derive(Clone)]
pub struct Var {
    space: Space,
    idx: u32,
}

impl Var {
    pub fn name(&self) -> String {
        self.space.lock().vars[self.idx as usize].name.clone()
    }

    pub fn options(&self) -> Vec<String> {
        self.space.lock().vars[self.idx as usize].options.clone()
    }

    /// The worlds where this choice lands on `option`. Never, if it isn't one.
    pub fn is(&self, option: &str) -> Bool {
        let mut m = self.space.lock();
        let Some(i) = m.vars[self.idx as usize].options.iter().position(|o| o == option) else {
            return Bool { space: self.space.clone(), f: FALSE };
        };
        let n = m.vars[self.idx as usize].options.len();
        let kids = (0..n).map(|k| if k == i { TRUE } else { FALSE }).collect();
        let f = m.mk(self.idx, kids);
        Bool { space: self.space.clone(), f }
    }

    /// Say this is one of several variables that are one draw, each for a different set of outcomes, and
    /// that it's the one in use where `in_use` holds. A die whose number of sides differs between worlds
    /// is a variable for each, and only one is ever rolled.
    pub fn in_group(&self, draw: &str, in_use: &Bool) {
        self.space.lock().vars[self.idx as usize].group = Some((draw.to_string(), in_use.f));
    }

    /// The draw this is one variable of, and the worlds where it's the one in use, if it is one of several.
    pub fn group(&self) -> Option<(String, Bool)> {
        let m = self.space.lock();
        m.vars[self.idx as usize].group.as_ref().map(|(name, f)| (name.clone(), Bool { space: self.space.clone(), f: *f }))
    }

    /// How likely each option is, in the order of `options`.
    pub fn weights(&self) -> Vec<f64> {
        self.space.lock().vars[self.idx as usize].weights.clone()
    }

    /// Whether each option is a gap, in the order of `options`.
    pub fn gaps(&self) -> Vec<bool> {
        self.space.lock().vars[self.idx as usize].gaps.clone()
    }

    /// This choice as a value.
    pub fn value(&self) -> Cond<String> {
        let cases = self.options().into_iter().map(|o| (self.is(&o).f, o)).collect();
        Cond { space: self.space.clone(), cases }
    }

    /// The options still possible where `given` holds.
    pub fn possible(&self, given: &Bool) -> Vec<String> {
        self.options().into_iter().filter(|o| !(&self.is(o) & given).is_never()).collect()
    }
}

/// One draw made of several variables, each for a different set of outcomes, with the worlds where each is
/// the one in use. It's one question: what the draw lands on.
#[derive(Clone)]
pub struct DrawGroup {
    pub name: String,
    pub variants: Vec<(Var, Bool)>,
}

impl DrawGroup {
    /// The variables among these that are one of several, gathered by the draw they're part of.
    pub fn gather(vars: impl IntoIterator<Item = Var>) -> Vec<DrawGroup> {
        let mut groups: BTreeMap<String, DrawGroup> = BTreeMap::new();
        for v in vars {
            if let Some((name, in_use)) = v.group() {
                let g = groups.entry(name.clone()).or_insert_with(|| DrawGroup { name, variants: Vec::new() });
                if !g.variants.iter().any(|(x, _)| x.idx == v.idx) {
                    g.variants.push((v, in_use));
                }
            }
        }
        groups.into_values().collect()
    }

    /// Everything it could land on, numbers in order.
    pub fn options(&self) -> Vec<String> {
        let mut all: Vec<String> = self.variants.iter().flat_map(|(v, _)| v.options()).collect();
        all.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.total_cmp(&y),
            _ => a.cmp(b),
        });
        all.dedup();
        all
    }

    /// The worlds where it lands on this.
    pub fn lands(&self, option: &str) -> Bool {
        let space = &self.variants[0].0.space;
        self.variants.iter().fold(space.never(), |acc, (v, in_use)| &acc | &(in_use & &v.is(option)))
    }

    /// Where each variable not in use is at rest, on its first option, so a world is counted once however
    /// many variables there are that it doesn't use.
    pub fn at_rest(&self) -> Bool {
        let space = &self.variants[0].0.space;
        self.variants.iter().fold(space.always(), |acc, (v, in_use)| {
            let first = v.options()[0].clone();
            &acc & &(in_use | &v.is(&first))
        })
    }
}

/// The exact set of worlds in which something holds.
#[derive(Clone)]
pub struct Bool {
    space: Space,
    f: F,
}

impl PartialEq for Bool {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f
    }
}

impl fmt::Debug for Bool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.show())
    }
}

impl std::ops::BitAnd for &Bool {
    type Output = Bool;
    fn bitand(self, o: &Bool) -> Bool {
        Bool { space: self.space.clone(), f: self.space.and(self.f, o.f) }
    }
}

impl std::ops::BitOr for &Bool {
    type Output = Bool;
    fn bitor(self, o: &Bool) -> Bool {
        Bool { space: self.space.clone(), f: self.space.or(self.f, o.f) }
    }
}

impl std::ops::Not for &Bool {
    type Output = Bool;
    fn not(self) -> Bool {
        Bool { space: self.space.clone(), f: self.space.not(self.f) }
    }
}

impl Bool {
    pub fn is_always(&self) -> bool {
        self.f == TRUE
    }

    pub fn is_never(&self) -> bool {
        self.f == FALSE
    }

    /// True everywhere `given` holds.
    pub fn holds_given(&self, given: &Bool) -> bool {
        (&!self & given).is_never()
    }

    /// True in every world where each choice lands on a named option.
    pub fn holds_among_known(&self) -> bool {
        fn go(m: &Mdd, f: F, seen: &mut BTreeSet<F>) -> bool {
            match f {
                FALSE => false,
                TRUE => true,
                _ if !seen.insert(f) => true,
                _ => {
                    let (var, kids) = &m.nodes[f as usize];
                    kids.iter().zip(&m.vars[*var as usize].gaps).all(|(k, gap)| *gap || go(m, *k, seen))
                }
            }
        }
        go(&self.space.lock(), self.f, &mut BTreeSet::new())
    }

    pub fn iff(&self, o: &Bool) -> Bool {
        let both = self & o;
        let neither = &!self & &!o;
        &both | &neither
    }

    pub fn implies(&self, o: &Bool) -> Bool {
        &!self | o
    }

    /// Probability under the option weights. Choices are independent of each other; paths that
    /// go through the same choice are exact, because each variable appears once.
    pub fn prob(&self) -> f64 {
        self.space.lock().prob(self.f, &mut HashMap::new())
    }

    pub fn prob_given(&self, given: &Bool) -> f64 {
        (self & given).prob() / given.prob()
    }

    /// How likely this is over the choices `chance` says are matters of chance, as a value that differs
    /// over the rest: for each way the rest could go, the chance it holds. `None` if that's too many
    /// different chances to list. With no matter of chance in it, it's certain where it holds and
    /// impossible elsewhere.
    pub fn weigh(&self, chance: &dyn Fn(&str) -> bool) -> Option<Cond<f64>> {
        /// The most different chances this lists.
        const MOST: usize = 4096;
        let mut m = self.space.lock();
        let support = m.support([self.f]);
        let flags: Vec<bool> = m.vars.iter().enumerate().map(|(i, v)| support.contains(&(i as u32)) && chance(&v.name)).collect();
        let cases = if flags.iter().any(|c| *c) {
            m.weigh(self.f, &flags, &mut HashMap::new(), MOST)?
        } else {
            let not = m.not(self.f);
            vec![(self.f, 1.0), (not, 0.0)]
        };
        Some(Cond { space: self.space.clone(), cases: cases.into_iter().filter(|(g, _)| *g != FALSE).collect() })
    }

    /// The worlds where some way these choices could go makes this hold. They no longer appear in it.
    pub fn exists(&self, vars: &[Var]) -> Bool {
        let set: BTreeSet<u32> = vars.iter().map(|v| v.idx).collect();
        Bool { space: self.space.clone(), f: self.space.lock().exists(self.f, &set, &mut HashMap::new()) }
    }

    /// The worlds where this holds, among those consistent with `picks`. The picked choices no longer
    /// appear in it.
    pub fn given(&self, picks: &Picks) -> Bool {
        Bool { space: self.space.clone(), f: self.space.lock().given(self.f, &picks.0, &mut HashMap::new()) }
    }

    /// The open choices this actually depends on.
    pub fn vars(&self) -> Vec<Var> {
        let support = self.space.lock().support([self.f]);
        support.into_iter().map(|idx| Var { space: self.space.clone(), idx }).collect()
    }

    /// The choices this actually depends on.
    pub fn support(&self) -> BTreeSet<String> {
        let m = self.space.lock();
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![self.f];
        while let Some(f) = stack.pop() {
            if f <= TRUE || !seen.insert(f) {
                continue;
            }
            let (var, kids) = &m.nodes[f as usize];
            out.insert(m.vars[*var as usize].name.clone());
            stack.extend(kids);
        }
        out
    }

    /// How many assignments of `vars` satisfy this, without listing them. `vars` must include
    /// everything this depends on.
    pub fn count(&self, vars: &[Var]) -> u128 {
        fn go(m: &Mdd, f: F, level: usize, order: &[u32], memo: &mut HashMap<(F, usize), u128>) -> u128 {
            if f == FALSE {
                return 0;
            }
            if level == order.len() {
                assert!(f == TRUE, "vars must include everything this depends on");
                return 1;
            }
            if let Some(&n) = memo.get(&(f, level)) {
                return n;
            }
            let v = order[level];
            let width = m.vars[v as usize].options.len();
            let n = (0..width).map(|i| go(m, m.cofactor(f, v, i), level + 1, order, memo)).sum();
            memo.insert((f, level), n);
            n
        }
        let mut order: Vec<u32> = vars.iter().map(|v| v.idx).collect();
        order.sort();
        order.dedup();
        go(&self.space.lock(), self.f, 0, &order, &mut HashMap::new())
    }

    /// How many assignments satisfy this, of the choices it depends on and `more`.
    pub fn count_with(&self, more: &[Var]) -> u128 {
        let mut vars = self.vars();
        vars.extend(more.iter().cloned());
        self.count(&vars)
    }

    /// This among the worlds where `given` holds: always if it holds in all of them, and never if in none.
    pub fn among(&self, given: &Bool) -> Bool {
        if self.holds_given(given) {
            self.space.always()
        } else if (self & given).is_never() {
            self.space.never()
        } else {
            self.clone()
        }
    }

    /// The satisfying assignments of `vars`, up to `limit`.
    pub fn solutions(&self, vars: &[Var], limit: usize) -> Vec<BTreeMap<String, String>> {
        fn go(m: &Mdd, f: F, level: usize, order: &[u32], at: &mut Vec<(String, String)>, out: &mut Vec<BTreeMap<String, String>>, limit: usize) {
            if f == FALSE || out.len() >= limit {
                return;
            }
            if level == order.len() {
                out.push(at.iter().cloned().collect());
                return;
            }
            let v = &m.vars[order[level] as usize];
            for (i, o) in v.options.iter().enumerate() {
                at.push((v.name.clone(), o.clone()));
                go(m, m.cofactor(f, order[level], i), level + 1, order, at, out, limit);
                at.pop();
            }
        }
        let mut order: Vec<u32> = vars.iter().map(|v| v.idx).collect();
        order.sort();
        let mut out = Vec::new();
        go(&self.space.lock(), self.f, 0, &order, &mut Vec::new(), &mut out, limit);
        out
    }

    /// True in one world: `choice → option`. Choices it doesn't depend on needn't be listed.
    pub fn at(&self, world: &BTreeMap<String, String>) -> bool {
        let m = self.space.lock();
        let mut f = self.f;
        while f > TRUE {
            let (var, kids) = &m.nodes[f as usize];
            let v = &m.vars[*var as usize];
            let chosen = world.get(&v.name).unwrap_or_else(|| panic!("world doesn't set {}", v.name));
            let i = v.options.iter().position(|o| o == chosen).unwrap_or_else(|| panic!("{} has no option {chosen}", v.name));
            f = kids[i];
        }
        f == TRUE
    }

    /// Readable logic, e.g. `calib∈{cad,free}` or `(root=a ∧ x=p) ∨ root=b`. Equal functions print
    /// the same.
    pub fn show(&self) -> String {
        self.show_with(&|s| s.to_string())
    }

    /// The same, with choices and options translated for reading, since they're usually handles.
    pub fn show_with(&self, label: &dyn Fn(&str) -> String) -> String {
        match self.f {
            FALSE => "never".into(),
            TRUE => "always".into(),
            f => {
                let space = self.space.lock();
                let terms = space.terms(f, label, MOST_TERMS);
                // An "and" alone needs no brackets, and nor does a term of one.
                let bracket = |t: &String| if terms.len() > 1 && t.contains(" ∧ ") { format!("({t})") } else { t.clone() };
                let mut text = terms.iter().map(bracket).collect::<Vec<_>>().join(" ∨ ");
                if terms.len() == MOST_TERMS {
                    let more = space.count_terms(f, &mut HashMap::new()) - MOST_TERMS as u128;
                    if more > 0 {
                        text.push_str(&format!(" ∨ … {more} more way{}", if more == 1 { "" } else { "s" }));
                    }
                }
                text
            }
        }
    }
}

/// A value that differs between worlds: mutually exclusive guards covering every world, with
/// equal values merged. Cost grows with the number of distinct values.
#[derive(Clone)]
pub struct Cond<T> {
    space: Space,
    cases: Vec<(F, T)>,
}

/// The same function whichever order its cases are held in.
impl<T: PartialEq> PartialEq for Cond<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cases.len() == other.cases.len() && self.cases.iter().all(|c| other.cases.contains(c))
    }
}

impl<T: fmt::Debug> fmt::Debug for Cond<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let cases: Vec<String> = self.cases().iter().map(|(g, v)| format!("{v:?} if {}", g.show())).collect();
        write!(f, "{}", cases.join(", "))
    }
}

impl<T> Cond<T> {
    /// What it comes to where `guard` holds, case by case: a case that holds nowhere there is gone. Unlike
    /// `among`, the guards then cover only those worlds.
    pub fn within(&self, guard: &Bool) -> Vec<(Bool, &T)> {
        self.cases.iter().filter_map(|(g, v)| {
            let f = self.space.and(*g, guard.f);
            (f != FALSE).then(|| (Bool { space: self.space.clone(), f }, v))
        }).collect()
    }

    /// How many different values it has across worlds.
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// The open choices this actually depends on.
    pub fn vars(&self) -> Vec<Var> {
        let support = self.space.lock().support(self.cases.iter().map(|c| c.0));
        support.into_iter().map(|idx| Var { space: self.space.clone(), idx }).collect()
    }

    pub fn cases(&self) -> Vec<(Bool, &T)> {
        self.cases.iter().map(|(g, v)| (Bool { space: self.space.clone(), f: *g }, v)).collect()
    }

    /// The value in one world.
    pub fn at(&self, world: &BTreeMap<String, String>) -> &T {
        self.cases().into_iter().find(|(g, _)| g.at(world)).expect("guards cover every world").1
    }

    fn push(cases: &mut Vec<(F, T)>, space: &Space, g: F, v: T)
    where
        T: PartialEq,
    {
        if g == FALSE {
            return;
        }
        match cases.iter_mut().find(|(_, x)| *x == v) {
            Some((existing, _)) => *existing = space.or(*existing, g),
            None => cases.push((g, v)),
        }
    }

    pub fn map<U: PartialEq>(&self, f: impl Fn(&T) -> U) -> Cond<U> {
        let mut cases = Vec::new();
        for (g, v) in &self.cases {
            Cond::push(&mut cases, &self.space, *g, f(v));
        }
        Cond { space: self.space.clone(), cases }
    }

    pub fn zip<B, U: PartialEq>(&self, other: &Cond<B>, f: impl Fn(&T, &B) -> U) -> Cond<U> {
        let mut cases = Vec::new();
        for (ga, a) in &self.cases {
            for (gb, b) in &other.cases {
                let g = self.space.and(*ga, *gb);
                if g != FALSE {
                    Cond::push(&mut cases, &self.space, g, f(a, b));
                }
            }
        }
        Cond { space: self.space.clone(), cases }
    }

    /// The worlds where a predicate of the value holds.
    pub fn holds(&self, pred: impl Fn(&T) -> bool) -> Bool {
        let f = self.cases.iter().filter(|(_, v)| pred(v)).fold(FALSE, |acc, (g, _)| self.space.or(acc, *g));
        Bool { space: self.space.clone(), f }
    }
}

impl<T: PartialEq + Clone> Cond<T> {
    /// This value among the worlds consistent with `picks`, so with any case they rule out gone.
    pub fn given(&self, picks: &Picks) -> Cond<T> {
        let mut cases = Vec::new();
        let mut memo = HashMap::new();
        for (g, v) in &self.cases {
            let narrowed = self.space.lock().given(*g, &picks.0, &mut memo);
            Cond::push(&mut cases, &self.space, narrowed, v.clone());
        }
        Cond { space: self.space.clone(), cases }
    }

    /// This value among the worlds where `given` holds: a case that holds in none of them is gone. What
    /// it reads as in the worlds outside `given` no longer means anything.
    pub fn among(&self, given: &Bool) -> Cond<T> {
        let (kept, gone): (Vec<&(F, T)>, Vec<&(F, T)>) = self.cases.iter().partition(|(g, _)| self.space.and(*g, given.f) != FALSE);
        let Some((first, rest)) = kept.split_first() else { return self.clone() };
        // The guards still cover every world: the first case left takes the worlds of those that went.
        let guard = gone.iter().fold(first.0, |acc, (g, _)| self.space.or(acc, *g));
        let mut cases = vec![(guard, first.1.clone())];
        cases.extend(rest.iter().map(|c| (*c).clone()));
        Cond { space: self.space.clone(), cases }
    }

    /// `then` where `guard` holds, `otherwise` elsewhere.
    pub fn when(guard: &Bool, then: &Cond<T>, otherwise: &Cond<T>) -> Cond<T> {
        let space = guard.space.clone();
        let not = space.not(guard.f);
        let mut cases = Vec::new();
        for (side, c) in [(guard.f, then), (not, otherwise)] {
            for (g, v) in &c.cases {
                Cond::push(&mut cases, &space, space.and(side, *g), v.clone());
            }
        }
        Cond { space, cases }
    }
}

impl Cond<bool> {
    pub fn truth(&self) -> Bool {
        self.holds(|v| *v)
    }
}

impl Cond<f64> {
    pub fn expected(&self) -> f64 {
        self.cases().iter().map(|(g, v)| g.prob() * **v).sum()
    }

    pub fn min(&self) -> f64 {
        self.cases.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min)
    }

    pub fn max(&self) -> f64 {
        self.cases.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max)
    }
}

// ---------- JSON: a value stands alone, so it can cross to another language and come back ----------

#[derive(Serialize, Deserialize)]
struct VarJson {
    name: String,
    options: Vec<String>,
    weights: Vec<f64>,
    gaps: Vec<bool>,
}

/// The diagram as a table, which is linear in its size where listing the worlds isn't. In
/// `nodes`, each kid is 0 for false, 1 for true, or 2 + the index of an earlier node. Without
/// a table, `root` is 0 or 1.
#[derive(Serialize, Deserialize)]
struct BoolJson {
    vars: Vec<VarJson>,
    nodes: Vec<(usize, Vec<u32>)>,
    root: u32,
}

impl Serialize for Bool {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        fn walk(m: &Mdd, f: F, local: &mut HashMap<F, u32>, vars: &mut Vec<u32>, nodes: &mut Vec<(usize, Vec<u32>)>) -> u32 {
            if f <= TRUE {
                return f;
            }
            if let Some(&i) = local.get(&f) {
                return i;
            }
            let (var, kids) = &m.nodes[f as usize];
            let kids: Vec<u32> = kids.iter().map(|k| walk(m, *k, local, vars, nodes)).collect();
            let v = vars.iter().position(|x| x == var).unwrap_or_else(|| {
                vars.push(*var);
                vars.len() - 1
            });
            nodes.push((v, kids));
            let i = nodes.len() as u32 + 1;
            local.insert(f, i);
            i
        }
        let m = self.space.lock();
        let (mut vars, mut nodes) = (Vec::new(), Vec::new());
        let root = walk(&m, self.f, &mut HashMap::new(), &mut vars, &mut nodes);
        let vars = vars
            .iter()
            .map(|v| {
                let d = &m.vars[*v as usize];
                VarJson { name: d.name.clone(), options: d.options.clone(), weights: d.weights.clone(), gaps: d.gaps.clone() }
            })
            .collect();
        BoolJson { vars, nodes, root }.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Bool {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let j = BoolJson::deserialize(de)?;
        j.build(&Space::global()).map_err(D::Error::custom)
    }
}

impl Bool {
    /// Read a value into a particular space, rather than the global one.
    pub fn from_json_in(space: &Space, v: &Value) -> Result<Bool, String> {
        serde_json::from_value::<BoolJson>(v.clone()).map_err(|e| e.to_string())?.build(space)
    }
}

impl BoolJson {
    fn build(self, space: &Space) -> Result<Bool, String> {
        let vars: Vec<Var> = self.vars.into_iter().map(|v| space.var_exact(&v.name, v.options, v.weights, v.gaps)).collect();
        let mut built: Vec<Bool> = vec![space.never(), space.always()];
        for (var, kids) in self.nodes {
            let var = vars.get(var).ok_or("no such variable")?;
            let options = var.options();
            if kids.len() != options.len() {
                return Err("a node needs one kid per option".into());
            }
            // Rebuilt by logic rather than copied, because this variable order may differ.
            let mut acc = space.never();
            for (kid, option) in kids.iter().zip(&options) {
                let kid = built.get(*kid as usize).ok_or("a kid must come before its parent")?;
                acc = &acc | &(&var.is(option) & kid);
            }
            built.push(acc);
        }
        built.get(self.root as usize).cloned().ok_or_else(|| "no such root".to_string())
    }
}

#[derive(Serialize, Deserialize)]
struct CaseJson<T> {
    when: Bool,
    value: T,
}

impl<T: Serialize> Serialize for Cond<T> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let cases: Vec<CaseJson<&T>> = self.cases.iter().map(|(g, value)| CaseJson { when: Bool { space: self.space.clone(), f: *g }, value }).collect();
        cases.serialize(ser)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Cond<T> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let cases = Vec::<CaseJson<T>>::deserialize(de)?;
        Ok(Cond { space: Space::global(), cases: cases.into_iter().map(|c| (c.when.f, c.value)).collect() })
    }
}

/// A value that a person may have typed in or a formula may have worked out: plain when it's the
/// same in every world, and a `Cond` when it isn't. So a file stays plain when it can be.
#[derive(Debug, Clone, PartialEq)]
pub struct Worldly<T>(pub Cond<T>);

impl<T> Worldly<T> {
    pub fn plain(v: T) -> Self {
        Worldly(Space::global().pure(v))
    }
}

impl<T: Serialize> Serialize for Worldly<T> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self.0.cases.as_slice() {
            [(_, only)] => only.serialize(ser),
            _ => self.0.serialize(ser),
        }
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Worldly<T> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(de)?;
        let is_cases = v.as_array().is_some_and(|cases| !cases.is_empty() && cases.iter().all(|c| c.as_object().is_some_and(|o| o.len() == 2 && o.contains_key("when") && o.contains_key("value"))));
        if is_cases {
            return serde_json::from_value(v).map(Worldly).map_err(D::Error::custom);
        }
        serde_json::from_value(v).map(Worldly::plain).map_err(D::Error::custom)
    }
}
