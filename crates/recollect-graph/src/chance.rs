//! Numbers and truths that are uncertain, as ordinary value types. See `docs/distributions.md`.
//!
//! A world is one way every open choice lands and one draw of everything uncertain. A `Rand` is a
//! number, or a truth, as a function of those draws: an expression over them, held in one table
//! shared by every value. The same expression always gets the same handle, so two compare equal in
//! constant time and a memoised cell stops what reads it from rerunning when its expression hasn't
//! changed, as with a `Bool`.
//!
//! The expression is exact. What's read from it is exact where there's a law for it, and is otherwise
//! read by drawing, which says so: see `Reader`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

type X = u32;

/// How many draws something is read from, when there's no law for it.
pub const DRAWS: usize = 4096;
/// How many bars a histogram has.
const BARS: usize = 32;
/// The most draws a sum can be over and still be recognised as having a law.
const MOST_ATOMS: usize = 4096;

/// A standard draw, with nothing to say about it but its shape. Every distribution is an expression
/// over these, so its parameters can be anything a number can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Std {
    /// Anywhere between 0 and 1, equally.
    Uniform,
    /// A bell curve about 0, with a spread of 1.
    Normal,
    /// A wait that averages 1.
    Exponential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Un {
    Neg,
    Exp,
    Ln,
    Sqrt,
    Floor,
    Ceil,
    Not,
    Defined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Max,
    Min,
    /// To this many decimal places.
    Round,
    Lt,
    Le,
    Eq,
    Ne,
    And,
    Or,
}

impl Op {
    fn commutes(self) -> bool {
        matches!(self, Op::Add | Op::Mul | Op::Max | Op::Min | Op::Eq | Op::Ne | Op::And | Op::Or)
    }

    fn gives_truth(self) -> bool {
        matches!(self, Op::Lt | Op::Le | Op::Eq | Op::Ne | Op::And | Op::Or)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Node {
    /// A number, as its bits. Undefined is the one NaN.
    Num(u64),
    Flag(bool),
    Draw(Std, u32),
    Un(Un, X),
    Bin(Op, X, X),
    /// A three-point estimate: a uniform draw, then the least, the likeliest and the most.
    Tri(X, X, X, X),
    If(X, X, X),
}

/// What's known about an expression without drawing anything.
#[derive(Debug, Clone, Copy)]
struct Info {
    /// True or false, where the rest are numbers.
    truth: bool,
    /// Defined in every draw. This takes arithmetic on what's defined to be defined, which it is short
    /// of overflowing.
    total: bool,
    /// It's never below this, or above that.
    lo: f64,
    hi: f64,
}

const ANYTHING: (f64, f64) = (f64::NEG_INFINITY, f64::INFINITY);

// ---------- what each part means, in one draw ----------
//
// Undefined is NaN, and a truth is 1 or 0. These are what both simplifying and drawing use, so they
// can't disagree, and they're what the formula language does to plain numbers.

fn tidy(x: f64) -> f64 {
    if x.is_finite() { x } else { f64::NAN }
}

fn apply_un(op: Un, x: f64) -> f64 {
    if op == Un::Defined {
        return if x.is_nan() { 0.0 } else { 1.0 };
    }
    if x.is_nan() {
        return f64::NAN;
    }
    tidy(match op {
        Un::Neg => -x,
        Un::Exp => x.exp(),
        Un::Ln => x.ln(),
        Un::Sqrt => x.sqrt(),
        Un::Floor => x.floor(),
        Un::Ceil => x.ceil(),
        Un::Not => 1.0 - x,
        Un::Defined => unreachable!("answered above"),
    })
}

fn apply_bin(op: Op, x: f64, y: f64) -> f64 {
    let either_undefined = x.is_nan() || y.is_nan();
    // Three-valued, so false && undefined is false, and true || undefined is true.
    match op {
        Op::And => return if x == 0.0 || y == 0.0 { 0.0 } else if either_undefined { f64::NAN } else { 1.0 },
        Op::Or => return if x == 1.0 || y == 1.0 { 1.0 } else if either_undefined { f64::NAN } else { 0.0 },
        _ => {}
    }
    if either_undefined {
        return f64::NAN;
    }
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    tidy(match op {
        Op::Add => x + y,
        Op::Sub => x - y,
        Op::Mul => x * y,
        Op::Div => x / y,
        Op::Pow => x.powf(y),
        Op::Max => x.max(y),
        Op::Min => x.min(y),
        Op::Round => {
            let scale = 10f64.powf(y);
            (x * scale).round() / scale
        }
        Op::Lt => flag(x < y),
        Op::Le => flag(x <= y),
        Op::Eq => flag(x == y),
        Op::Ne => flag(x != y),
        Op::And | Op::Or => unreachable!("answered above"),
    })
}

fn apply_tri(u: f64, low: f64, likely: f64, high: f64) -> f64 {
    if u.is_nan() || !(low <= likely && likely <= high) {
        return f64::NAN;
    }
    if low == high {
        return low;
    }
    let width = high - low;
    if u < (likely - low) / width { low + (u * width * (likely - low)).sqrt() } else { high - ((1.0 - u) * width * (high - likely)).sqrt() }
}

fn apply_if(c: f64, a: f64, b: f64) -> f64 {
    if c.is_nan() {
        f64::NAN
    } else if c != 0.0 {
        a
    } else {
        b
    }
}

/// A standard draw, from a number between 0 and 1.
fn apply_std(std: Std, u: f64) -> f64 {
    match std {
        Std::Uniform => u,
        Std::Normal => probit(u),
        Std::Exponential => -(-u).ln_1p(),
    }
}

/// Where a standard bell curve has this much of its weight below it. Acklam's rational approximation,
/// which is right to about one part in a billion.
pub fn probit(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    const B: [f64; 5] = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    const C: [f64; 6] = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    const D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    const LOW: f64 = 0.02425;
    if !(p > 0.0 && p < 1.0) {
        return f64::NAN;
    }
    let tail = |q: f64| (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    if p < LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - LOW {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// How much of a standard bell curve is below this. The series near the middle and the continued
/// fraction further out are each right to about as many figures as a number holds. It's never quite 0
/// or 1, because nothing's impossible for a bell curve.
pub fn phi(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let a = x.abs();
    let density = (-a * a / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    // How much is beyond `a`, on one side.
    let beyond = if a < 3.0 {
        let (mut sum, mut term, mut k) = (a, a, 1.0);
        loop {
            term *= a * a / (2.0 * k + 1.0);
            if sum + term == sum {
                break;
            }
            sum += term;
            k += 1.0;
        }
        0.5 - sum * density
    } else {
        let mut t = a;
        for k in (1..=200).rev() {
            t = a + f64::from(k) / t;
        }
        density / t
    };
    let below = if x < 0.0 { beyond } else { 1.0 - beyond };
    below.clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON / 2.0)
}

// ---------- the table ----------

#[derive(Default)]
struct Arena {
    draws: Vec<String>,
    draw_index: HashMap<String, u32>,
    nodes: Vec<Node>,
    info: Vec<Info>,
    unique: HashMap<Node, X>,
}

static ARENA: LazyLock<Mutex<Arena>> = LazyLock::new(|| Mutex::new(Arena::default()));

fn arena() -> MutexGuard<'static, Arena> {
    ARENA.lock().unwrap_or_else(PoisonError::into_inner)
}

fn bits(v: f64) -> u64 {
    // One NaN, and one zero, so equal numbers are one node.
    if v.is_nan() {
        f64::NAN.to_bits()
    } else if v == 0.0 {
        0f64.to_bits()
    } else {
        v.to_bits()
    }
}

/// The least and most of a product, or of anything else that's one of the four corners.
fn corners(f: impl Fn(f64, f64) -> f64, a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let all = [f(a.0, b.0), f(a.0, b.1), f(a.1, b.0), f(a.1, b.1)];
    if all.iter().any(|x| x.is_nan()) {
        return ANYTHING;
    }
    (all.iter().copied().fold(f64::INFINITY, f64::min), all.iter().copied().fold(f64::NEG_INFINITY, f64::max))
}

/// One side of a comparison with a plain number: what it says about the other side.
struct Bound {
    of: X,
    at: f64,
    /// It's below `at`, where otherwise it's above.
    below: bool,
    /// It can't be `at` itself.
    strict: bool,
}

impl Arena {
    fn mk(&mut self, node: Node, info: Info) -> X {
        if let Some(&x) = self.unique.get(&node) {
            return x;
        }
        let x = self.nodes.len() as X;
        self.nodes.push(node.clone());
        self.info.push(info);
        self.unique.insert(node, x);
        x
    }

    fn num(&mut self, v: f64) -> X {
        let v = tidy(v);
        let (lo, hi) = if v.is_nan() { ANYTHING } else { (v, v) };
        self.mk(Node::Num(bits(v)), Info { truth: false, total: !v.is_nan(), lo, hi })
    }

    fn flag(&mut self, b: bool) -> X {
        let v = if b { 1.0 } else { 0.0 };
        self.mk(Node::Flag(b), Info { truth: true, total: true, lo: v, hi: v })
    }

    /// A plain answer, as a truth or a number. Undefined is neither.
    fn lit(&mut self, v: f64, truth: bool) -> X {
        if v.is_nan() {
            self.num(f64::NAN)
        } else if truth {
            self.flag(v != 0.0)
        } else {
            self.num(v)
        }
    }

    fn draw(&mut self, std: Std, name: &str) -> X {
        let i = match self.draw_index.get(name) {
            Some(&i) => i,
            None => {
                let i = self.draws.len() as u32;
                self.draws.push(name.to_string());
                self.draw_index.insert(name.to_string(), i);
                i
            }
        };
        let (lo, hi) = match std {
            Std::Uniform => (0.0, 1.0),
            Std::Normal => ANYTHING,
            Std::Exponential => (0.0, f64::INFINITY),
        };
        self.mk(Node::Draw(std, i), Info { truth: false, total: true, lo, hi })
    }

    /// A plain value, if that's all this is. A truth is 1 or 0, and undefined is NaN.
    fn plain(&self, x: X) -> Option<f64> {
        match self.nodes[x as usize] {
            Node::Num(b) => Some(f64::from_bits(b)),
            Node::Flag(b) => Some(if b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn is(&self, x: X, v: f64) -> bool {
        self.nodes[x as usize] == Node::Num(bits(v))
    }

    fn span(&self, x: X) -> (f64, f64) {
        let i = &self.info[x as usize];
        (i.lo, i.hi)
    }

    fn total(&self, x: X) -> bool {
        self.info[x as usize].total
    }

    fn un(&mut self, op: Un, a: X) -> X {
        let truth = matches!(op, Un::Not | Un::Defined);
        if let Some(v) = self.plain(a) {
            return self.lit(apply_un(op, v), truth);
        }
        match (op, &self.nodes[a as usize]) {
            (Un::Neg, Node::Un(Un::Neg, inner)) | (Un::Not, Node::Un(Un::Not, inner)) => return *inner,
            (Un::Defined, _) if self.total(a) => return self.flag(true),
            _ => {}
        }
        let (lo, hi) = self.span(a);
        let t = self.total(a);
        let (total, (lo, hi)) = match op {
            Un::Neg => (t, (-hi, -lo)),
            Un::Exp => (t && hi < 700.0, (lo.exp(), hi.exp())),
            Un::Ln => (t && lo > 0.0, (if lo > 0.0 { lo.ln() } else { f64::NEG_INFINITY }, if hi > 0.0 { hi.ln() } else { f64::NEG_INFINITY })),
            Un::Sqrt => (t && lo >= 0.0, (lo.max(0.0).sqrt(), hi.max(0.0).sqrt())),
            Un::Floor => (t, (lo.floor(), hi.floor())),
            Un::Ceil => (t, (lo.ceil(), hi.ceil())),
            Un::Not => (t, (0.0, 1.0)),
            Un::Defined => (true, (0.0, 1.0)),
        };
        self.mk(Node::Un(op, a), Info { truth, total, lo, hi })
    }

    /// What a comparison says about one of its sides, if the other is a plain number.
    fn bound(&self, x: X) -> Option<Bound> {
        let Node::Bin(op @ (Op::Lt | Op::Le), l, r) = self.nodes[x as usize] else { return None };
        let strict = op == Op::Lt;
        match (self.plain(l), self.plain(r)) {
            (None, Some(at)) if !at.is_nan() => Some(Bound { of: l, at, below: true, strict }),
            (Some(at), None) if !at.is_nan() => Some(Bound { of: r, at, below: false, strict }),
            _ => None,
        }
    }

    fn bin(&mut self, op: Op, a: X, b: X) -> X {
        if let (Some(x), Some(y)) = (self.plain(a), self.plain(b)) {
            return self.lit(apply_bin(op, x, y), op.gives_truth());
        }
        // Undefined on either side of anything but a truth is undefined, whatever the other side is.
        let undefined = |x: X| self.plain(x).is_some_and(f64::is_nan);
        if !matches!(op, Op::And | Op::Or) && (undefined(a) || undefined(b)) {
            return self.num(f64::NAN);
        }
        let (a, b) = if op.commutes() && a > b { (b, a) } else { (a, b) };
        let both = self.total(a) && self.total(b);
        let ((al, ah), (bl, bh)) = (self.span(a), self.span(b));
        match op {
            Op::Add if self.is(a, 0.0) => return b,
            Op::Add | Op::Sub if self.is(b, 0.0) => return a,
            Op::Sub if a == b && both => return self.num(0.0),
            Op::Mul if self.is(a, 1.0) => return b,
            Op::Mul | Op::Div if self.is(b, 1.0) => return a,
            Op::Mul if (self.is(a, 0.0) || self.is(b, 0.0)) && both => return self.num(0.0),
            Op::Max | Op::Min | Op::And | Op::Or if a == b => return a,
            Op::Max if both && al >= bh => return a,
            Op::Max if both && bl >= ah => return b,
            Op::Min if both && ah <= bl => return a,
            Op::Min if both && bh <= al => return b,
            Op::Lt if both && ah < bl => return self.flag(true),
            Op::Lt if both && (al >= bh || a == b) => return self.flag(false),
            Op::Le if both && (ah <= bl || a == b) => return self.flag(true),
            Op::Le if both && al > bh => return self.flag(false),
            Op::Eq if both && a == b => return self.flag(true),
            Op::Eq if both && (ah < bl || bh < al) => return self.flag(false),
            Op::Ne if both && a == b => return self.flag(false),
            Op::Ne if both && (ah < bl || bh < al) => return self.flag(true),
            Op::And if self.nodes[a as usize] == Node::Flag(false) || self.nodes[b as usize] == Node::Flag(false) => return self.flag(false),
            Op::And if self.nodes[a as usize] == Node::Flag(true) => return b,
            Op::And if self.nodes[b as usize] == Node::Flag(true) => return a,
            Op::Or if self.nodes[a as usize] == Node::Flag(true) || self.nodes[b as usize] == Node::Flag(true) => return self.flag(true),
            Op::Or if self.nodes[a as usize] == Node::Flag(false) => return b,
            Op::Or if self.nodes[b as usize] == Node::Flag(false) => return a,
            _ => {}
        }
        // Two limits on one number that leave nothing between them: x > 5 && x < 3.
        if op == Op::And
            && let (Some(p), Some(q)) = (self.bound(a), self.bound(b))
            && p.of == q.of
            && p.below != q.below
            && self.total(p.of)
        {
            let (under, over) = if p.below { (p, q) } else { (q, p) };
            if over.at > under.at || (over.at == under.at && (over.strict || under.strict)) {
                return self.flag(false);
            }
        }
        // Two limits on one number that between them leave nothing out: x > 1 || x <= 1.
        if op == Op::Or
            && let (Some(p), Some(q)) = (self.bound(a), self.bound(b))
            && p.of == q.of
            && p.below != q.below
            && self.total(p.of)
        {
            let (under, over) = if p.below { (p, q) } else { (q, p) };
            if over.at < under.at || (over.at == under.at && !(over.strict && under.strict)) {
                return self.flag(true);
            }
        }
        let truth = op.gives_truth();
        let (total, span) = match op {
            Op::Add => (both, (al + bl, ah + bh)),
            Op::Sub => (both, (al - bh, ah - bl)),
            Op::Mul => (both, corners(|x, y| x * y, (al, ah), (bl, bh))),
            Op::Div if bl > 0.0 || bh < 0.0 => (both, corners(|x, y| x / y, (al, ah), (bl, bh))),
            Op::Div => (false, ANYTHING),
            // A whole power of anything is defined, and any power of what's above 0.
            Op::Pow => (both && (al > 0.0 || self.plain(b).is_some_and(|p| p.fract() == 0.0 && p >= 0.0)), ANYTHING),
            Op::Max => (both, (al.max(bl), ah.max(bh))),
            Op::Min => (both, (al.min(bl), ah.min(bh))),
            Op::Round => (both, ANYTHING),
            Op::Lt | Op::Le | Op::Eq | Op::Ne | Op::And | Op::Or => (both, (0.0, 1.0)),
        };
        let (lo, hi) = if span.0.is_nan() || span.1.is_nan() { ANYTHING } else { span };
        self.mk(Node::Bin(op, a, b), Info { truth, total, lo, hi })
    }

    fn tri(&mut self, u: X, low: X, likely: X, high: X) -> X {
        let plain = [self.plain(low), self.plain(likely), self.plain(high)];
        if let (Some(u), [Some(a), Some(b), Some(c)]) = (self.plain(u), plain) {
            return self.num(apply_tri(u, a, b, c));
        }
        let total = self.total(u) && matches!(plain, [Some(a), Some(b), Some(c)] if a <= b && b <= c);
        let (lo, hi) = (self.span(low).0, self.span(high).1);
        self.mk(Node::Tri(u, low, likely, high), Info { truth: false, total, lo, hi })
    }

    fn pick(&mut self, c: X, a: X, b: X) -> X {
        if let Some(v) = self.plain(c) {
            return if v.is_nan() {
                self.num(f64::NAN)
            } else if v != 0.0 {
                a
            } else {
                b
            };
        }
        if a == b && self.total(c) {
            return a;
        }
        let (ia, ib) = (self.info[a as usize], self.info[b as usize]);
        let info = Info { truth: ia.truth || ib.truth, total: self.total(c) && ia.total && ib.total, lo: ia.lo.min(ib.lo), hi: ia.hi.max(ib.hi) };
        self.mk(Node::If(c, a, b), info)
    }

    fn intern(&mut self, t: &Term) -> X {
        match t {
            Term::Undefined => self.num(f64::NAN),
            Term::Num(n) => self.num(*n),
            Term::Flag(b) => self.flag(*b),
            Term::Rand(r) => r.0,
        }
    }

    fn out(&self, x: X) -> Term {
        match self.nodes[x as usize] {
            Node::Num(b) if f64::from_bits(b).is_nan() => Term::Undefined,
            Node::Num(b) => Term::Num(f64::from_bits(b)),
            Node::Flag(b) => Term::Flag(b),
            _ => Term::Rand(Rand(x)),
        }
    }

    fn kids(&self, x: X) -> Vec<X> {
        match self.nodes[x as usize] {
            Node::Num(_) | Node::Flag(_) | Node::Draw(..) => vec![],
            Node::Un(_, a) => vec![a],
            Node::Bin(_, a, b) => vec![a, b],
            Node::If(a, b, c) => vec![a, b, c],
            Node::Tri(a, b, c, d) => vec![a, b, c, d],
        }
    }

    /// Everything `root` is made of that `skip` doesn't already have, each after what it's made of.
    /// Parts are always made before what uses them, so that's the order they were made in.
    fn reach(&self, root: X, skip: impl Fn(X) -> bool) -> Vec<(X, Node)> {
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = vec![root];
        while let Some(x) = stack.pop() {
            if skip(x) || !seen.insert(x) {
                continue;
            }
            stack.extend(self.kids(x));
        }
        seen.into_iter().map(|x| (x, self.nodes[x as usize].clone())).collect()
    }
}

/// A number or a truth that may be uncertain: what goes into the table, and what comes out.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Undefined,
    Num(f64),
    Flag(bool),
    Rand(Rand),
}

/// A number, or a truth, that differs from draw to draw.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rand(X);

impl fmt::Debug for Rand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "~{}", self.0)
    }
}

/// A standard draw with this name. The same name is the same draw, wherever it's asked for.
pub fn draw(std: Std, name: &str) -> Rand {
    Rand(arena().draw(std, name))
}

pub fn un(op: Un, a: &Term) -> Term {
    let mut t = arena();
    let a = t.intern(a);
    let x = t.un(op, a);
    t.out(x)
}

pub fn bin(op: Op, a: &Term, b: &Term) -> Term {
    let mut t = arena();
    let (a, b) = (t.intern(a), t.intern(b));
    let x = t.bin(op, a, b);
    t.out(x)
}

/// A three-point estimate, from a uniform draw.
pub fn tri(u: &Term, low: &Term, likely: &Term, high: &Term) -> Term {
    let mut t = arena();
    let (u, low, likely, high) = (t.intern(u), t.intern(low), t.intern(likely), t.intern(high));
    let x = t.tri(u, low, likely, high);
    t.out(x)
}

/// `a` where `c` is true, `b` where it's false, and undefined where it's undefined.
pub fn pick(c: &Term, a: &Term, b: &Term) -> Term {
    let mut t = arena();
    let (c, a, b) = (t.intern(c), t.intern(a), t.intern(b));
    let x = t.pick(c, a, b);
    t.out(x)
}

/// How many expressions there are: a measure of how hard things have been.
pub fn size() -> usize {
    arena().nodes.len()
}

impl Rand {
    /// True or false, where otherwise it's a number.
    pub fn is_truth(&self) -> bool {
        arena().info[self.0 as usize].truth
    }

    /// Defined in every draw.
    pub fn is_total(&self) -> bool {
        arena().total(self.0)
    }

    /// It's never below the first of these, or above the second. Either can be infinite.
    pub fn bounds(&self) -> (f64, f64) {
        arena().span(self.0)
    }

    /// The names of the draws it depends on.
    pub fn draws(&self) -> Vec<String> {
        let t = arena();
        t.reach(self.0, |_| false).into_iter().filter_map(|(_, n)| if let Node::Draw(_, i) = n { Some(t.draws[i as usize].clone()) } else { None }).collect()
    }

    /// Its value in one draw, given where between 0 and 1 each named draw falls. Undefined is `None`,
    /// and a truth is 1 or 0.
    pub fn at(&self, draws: &dyn Fn(&str) -> f64) -> Option<f64> {
        let (parts, names) = {
            let t = arena();
            (t.reach(self.0, |_| false), t.draws.clone())
        };
        let mut values: HashMap<X, f64> = HashMap::with_capacity(parts.len());
        for (x, node) in parts {
            let v = match node {
                Node::Num(b) => f64::from_bits(b),
                Node::Flag(b) => f64::from(u8::from(b)),
                Node::Draw(std, i) => apply_std(std, draws(&names[i as usize])),
                Node::Un(op, a) => apply_un(op, values[&a]),
                Node::Bin(op, a, b) => apply_bin(op, values[&a], values[&b]),
                Node::Tri(u, a, b, c) => apply_tri(values[&u], values[&a], values[&b], values[&c]),
                Node::If(c, a, b) => apply_if(values[&c], values[&a], values[&b]),
            };
            values.insert(x, v);
        }
        Some(values[&self.0]).filter(|v| !v.is_nan())
    }
}

/// A number that may be uncertain, for a cell that holds one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Num {
    Plain(f64),
    Rand(Rand),
}

impl Num {
    /// The number, if it's a plain one.
    pub fn plain(&self) -> Option<f64> {
        match self {
            Num::Plain(n) => Some(*n),
            Num::Rand(_) => None,
        }
    }

    pub fn term(&self) -> Term {
        match self {
            Num::Plain(n) => Term::Num(*n),
            Num::Rand(r) => Term::Rand(*r),
        }
    }
}

impl crate::worlds::Cond<Option<Num>> {
    /// The same, as plain numbers, for what's known to have nothing uncertain in it. It panics if
    /// something does.
    pub fn plain(&self) -> crate::worlds::Cond<Option<f64>> {
        self.map(|n| n.as_ref().map(|n| n.plain().expect("nothing uncertain here")))
    }
}

impl From<f64> for Num {
    fn from(n: f64) -> Num {
        Num::Plain(n)
    }
}

impl PartialEq<f64> for Num {
    fn eq(&self, other: &f64) -> bool {
        self.plain() == Some(*other)
    }
}

// ---------- JSON: an expression stands alone, as a `Bool` does ----------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NodeJson {
    /// `null` is undefined.
    Num(Option<f64>),
    Flag(bool),
    Draw(Std, usize),
    Un(Un, usize),
    Bin(Op, usize, usize),
    Tri(usize, usize, usize, usize),
    If(usize, usize, usize),
}

/// The expression as a table, each part after what it's made of, so it's as long as the expression
/// is big however much of it is shared.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TableJson {
    draws: Vec<String>,
    nodes: Vec<NodeJson>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RandJson {
    rand: TableJson,
}

impl Serialize for Rand {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let t = arena();
        let parts = t.reach(self.0, |_| false);
        let local: HashMap<X, usize> = parts.iter().enumerate().map(|(i, (x, _))| (*x, i)).collect();
        let mut draws: Vec<String> = Vec::new();
        let nodes = parts
            .iter()
            .map(|(_, node)| match node {
                Node::Num(b) => NodeJson::Num(Some(f64::from_bits(*b)).filter(|v| !v.is_nan())),
                Node::Flag(b) => NodeJson::Flag(*b),
                Node::Draw(std, i) => {
                    let name = &t.draws[*i as usize];
                    let at = draws.iter().position(|d| d == name).unwrap_or_else(|| {
                        draws.push(name.clone());
                        draws.len() - 1
                    });
                    NodeJson::Draw(*std, at)
                }
                Node::Un(op, a) => NodeJson::Un(*op, local[a]),
                Node::Bin(op, a, b) => NodeJson::Bin(*op, local[a], local[b]),
                Node::Tri(u, a, b, c) => NodeJson::Tri(local[u], local[a], local[b], local[c]),
                Node::If(c, a, b) => NodeJson::If(local[c], local[a], local[b]),
            })
            .collect();
        RandJson { rand: TableJson { draws, nodes } }.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Rand {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let j = RandJson::deserialize(de)?.rand;
        let mut t = arena();
        // Made again by what each part means, so it's the same expression here whatever it was there.
        let mut built: Vec<X> = Vec::with_capacity(j.nodes.len());
        for node in j.nodes {
            let at = |i: usize| built.get(i).copied().ok_or_else(|| D::Error::custom("a part must come before what's made of it"));
            let x = match node {
                NodeJson::Num(v) => t.num(v.unwrap_or(f64::NAN)),
                NodeJson::Flag(b) => t.flag(b),
                NodeJson::Draw(std, i) => {
                    let name = j.draws.get(i).ok_or_else(|| D::Error::custom("no such draw"))?;
                    t.draw(std, name)
                }
                NodeJson::Un(op, a) => {
                    let a = at(a)?;
                    t.un(op, a)
                }
                NodeJson::Bin(op, a, b) => {
                    let (a, b) = (at(a)?, at(b)?);
                    t.bin(op, a, b)
                }
                NodeJson::Tri(u, a, b, c) => {
                    let (u, a, b, c) = (at(u)?, at(a)?, at(b)?, at(c)?);
                    t.tri(u, a, b, c)
                }
                NodeJson::If(c, a, b) => {
                    let (c, a, b) = (at(c)?, at(a)?, at(b)?);
                    t.pick(c, a, b)
                }
            };
            built.push(x);
        }
        built.last().map(|x| Rand(*x)).ok_or_else(|| D::Error::custom("an expression has something in it"))
    }
}

// ---------- laws: what an expression is exactly, where that has a name ----------

/// A distribution with a name, which can be read exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum Law {
    Normal { mean: f64, sd: f64 },
    Uniform { from: f64, to: f64 },
    /// A wait that starts at `from` and averages `mean` more.
    Exponential { from: f64, mean: f64 },
    /// A number whose log is a normal with this mean and spread.
    Lognormal { mu: f64, sigma: f64 },
    Triangular { low: f64, likely: f64, high: f64 },
}

/// A number as it's written for reading: four figures, and no more zeros than it needs.
pub fn figures(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    let places = (3 - x.abs().log10().floor() as i32).clamp(0, 12) as usize;
    let text = format!("{x:.places$}");
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

impl Law {
    pub fn mean(&self) -> f64 {
        match *self {
            Law::Normal { mean, .. } => mean,
            Law::Uniform { from, to } => (from + to) / 2.0,
            Law::Exponential { from, mean } => from + mean,
            Law::Lognormal { mu, sigma } => (mu + sigma * sigma / 2.0).exp(),
            Law::Triangular { low, likely, high } => (low + likely + high) / 3.0,
        }
    }

    pub fn sd(&self) -> f64 {
        match *self {
            Law::Normal { sd, .. } => sd,
            Law::Uniform { from, to } => (to - from) / 12f64.sqrt(),
            Law::Exponential { mean, .. } => mean,
            Law::Lognormal { mu, sigma } => (((sigma * sigma).exp() - 1.0) * (2.0 * mu + sigma * sigma).exp()).sqrt(),
            Law::Triangular { low: a, likely: b, high: c } => ((a * a + b * b + c * c - a * b - a * c - b * c) / 18.0).sqrt(),
        }
    }

    /// Where it has this much of its weight below it.
    pub fn quantile(&self, p: f64) -> f64 {
        match *self {
            Law::Normal { mean, sd } => mean + sd * probit(p),
            Law::Uniform { from, to } => from + (to - from) * p,
            Law::Exponential { from, mean } => from + mean * apply_std(Std::Exponential, p),
            Law::Lognormal { mu, sigma } => (mu + sigma * probit(p)).exp(),
            Law::Triangular { low, likely, high } => apply_tri(p, low, likely, high),
        }
    }

    /// How much of its weight is below this. Each of these is smooth, so that's the same as at or below.
    pub fn below(&self, x: f64) -> f64 {
        match *self {
            Law::Normal { mean, sd } => phi((x - mean) / sd),
            Law::Uniform { from, to } => ((x - from) / (to - from)).clamp(0.0, 1.0),
            Law::Exponential { from, .. } if x <= from => 0.0,
            // A wait is never certainly over, however long it's been.
            Law::Exponential { from, mean } => (-(-(x - from) / mean).exp_m1()).min(1.0 - f64::EPSILON / 2.0),
            Law::Lognormal { .. } if x <= 0.0 => 0.0,
            Law::Lognormal { mu, sigma } => phi((x.ln() - mu) / sigma),
            Law::Triangular { low, .. } if x <= low => 0.0,
            Law::Triangular { high, .. } if x >= high => 1.0,
            Law::Triangular { low, likely, high } if x <= likely => (x - low) * (x - low) / ((high - low) * (likely - low)),
            Law::Triangular { low, likely, high } => 1.0 - (high - x) * (high - x) / ((high - low) * (high - likely)),
        }
    }

    /// As it would be written in a formula.
    pub fn text(&self) -> String {
        match *self {
            Law::Normal { mean, sd } => format!("normal({}, {})", figures(mean), figures(sd)),
            Law::Uniform { from, to } => format!("uniform({}, {})", figures(from), figures(to)),
            Law::Exponential { from, mean } if from == 0.0 => format!("exponential({})", figures(mean)),
            Law::Exponential { from, mean } => format!("{} + exponential({})", figures(from), figures(mean)),
            Law::Lognormal { mu, sigma } => format!("lognormal({}, {})", figures(mu), figures(sigma)),
            Law::Triangular { low, likely, high } => format!("triangular({}, {}, {})", figures(low), figures(likely), figures(high)),
        }
    }
}

/// A plain number plus so much of each of some draws. Two of these add exactly, whatever they share,
/// which is why a law found this way is right about what depends on what.
#[derive(Debug, Clone)]
struct Straight {
    plain: f64,
    /// Each draw, and how much of it, in order.
    of: Vec<(X, f64)>,
}

impl Straight {
    fn scaled(&self, by: f64) -> Straight {
        Straight { plain: self.plain * by, of: self.of.iter().map(|(x, k)| (*x, k * by)).collect() }
    }

    fn plus(&self, other: &Straight) -> Option<Straight> {
        let mut of = Vec::with_capacity(self.of.len() + other.of.len());
        let (mut i, mut j) = (0, 0);
        while i < self.of.len() || j < other.of.len() {
            let take_left = j == other.of.len() || (i < self.of.len() && self.of[i].0 <= other.of[j].0);
            if take_left && j < other.of.len() && self.of[i].0 == other.of[j].0 {
                let k = self.of[i].1 + other.of[j].1;
                if k != 0.0 {
                    of.push((self.of[i].0, k));
                }
                i += 1;
                j += 1;
            } else if take_left {
                of.push(self.of[i]);
                i += 1;
            } else {
                of.push(other.of[j]);
                j += 1;
            }
        }
        (of.len() <= MOST_ATOMS).then_some(Straight { plain: self.plain + other.plain, of })
    }
}

// ---------- reading: by a law where there is one, and otherwise by drawing ----------

/// A number between 0 and 1 for each of `DRAWS` draws of something with this name. Each stretch of
/// 1/`DRAWS` has exactly one, in an order that's the name's own, so one draw's histogram is as smooth as
/// it can be, two draws are unrelated, and the same name always gives the same numbers.
fn uniforms(name: &str) -> Vec<f64> {
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let seed = name.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3));
    let mut order: Vec<usize> = (0..DRAWS).collect();
    order.sort_by_key(|i| mix(seed ^ (*i as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)));
    let mut out = vec![0.0; DRAWS];
    for (rank, i) in order.into_iter().enumerate() {
        let within = ((mix(seed.rotate_left(17) ^ i as u64) >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
        out[i] = (rank as f64 + within) / DRAWS as f64;
    }
    out
}

/// A histogram: how much of the weight falls in each of some equal stretches from one number to another.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hist {
    pub from: f64,
    pub to: f64,
    /// The share in each stretch. What's beyond either end is in the first or the last.
    pub bars: Vec<f64>,
}

/// What a number comes to across what's uncertain, and how that was got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub mean: f64,
    /// Its spread: the standard deviation.
    pub sd: f64,
    /// It's below this one time in twenty.
    pub low: f64,
    /// It's below this half the time.
    pub mid: f64,
    /// It's above this one time in twenty.
    pub high: f64,
    /// It's never below this, if that's known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub least: Option<f64>,
    /// It's never above this, if that's known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub most: Option<f64>,
    /// How much of the time it's undefined. The rest are about when it isn't.
    pub undefined: f64,
    pub hist: Hist,
    /// The distribution it is, if it has a name, as it would be written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub law: Option<String>,
    /// The numbers here are exactly right. Otherwise they were read from `draws` draws.
    pub exact: bool,
    pub draws: usize,
    /// How far out the mean could reasonably be: its standard error. 0 if it's exact.
    pub error: f64,
}

/// How likely a truth is, and how that was got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chance {
    /// How likely it is to be true, among the draws where it's defined.
    pub chance: f64,
    pub undefined: f64,
    pub exact: bool,
    pub draws: usize,
    /// In how many of the draws it was true.
    pub hits: usize,
    /// How far out the chance could reasonably be: its standard error. 0 if it's exact.
    pub error: f64,
}

/// One of the things a number could be, for a mixture: this much of the time, it's this.
pub struct Share {
    pub weight: f64,
    pub value: Term,
}

/// Reads expressions. It keeps what it's drawn, so reading many that share parts draws each part once:
/// make one for each view, and ask it for everything that view shows.
#[derive(Default)]
pub struct Reader {
    uniforms: HashMap<u32, Arc<Vec<f64>>>,
    columns: HashMap<X, Arc<Vec<f64>>>,
    straight: HashMap<X, Option<Arc<Straight>>>,
}

/// Values with how much each counts, in order.
struct Weighted(Vec<(f64, f64)>);

impl Weighted {
    fn new(mut values: Vec<(f64, f64)>) -> Weighted {
        values.retain(|(v, w)| !v.is_nan() && *w > 0.0);
        values.sort_by(|a, b| a.0.total_cmp(&b.0));
        Weighted(values)
    }

    fn total(&self) -> f64 {
        self.0.iter().map(|(_, w)| w).sum()
    }

    fn mean(&self) -> f64 {
        self.0.iter().map(|(v, w)| v * w).sum::<f64>() / self.total()
    }

    fn sd(&self) -> f64 {
        let mean = self.mean();
        (self.0.iter().map(|(v, w)| (v - mean) * (v - mean) * w).sum::<f64>() / self.total()).sqrt()
    }

    fn quantile(&self, p: f64) -> f64 {
        let target = p * self.total();
        let mut below = 0.0;
        for (v, w) in &self.0 {
            below += w;
            if below >= target {
                return *v;
            }
        }
        self.0.last().map_or(f64::NAN, |(v, _)| *v)
    }

    fn hist(&self) -> Hist {
        // The far tails would squash everything else into a bar or two.
        let (from, to) = (self.quantile(0.002), self.quantile(0.998));
        if !(to > from) {
            return Hist { from, to, bars: vec![1.0] };
        }
        let mut bars = vec![0.0; BARS];
        let total = self.total();
        for (v, w) in &self.0 {
            let at = (((v - from) / (to - from)) * BARS as f64).floor().clamp(0.0, BARS as f64 - 1.0) as usize;
            bars[at] += w / total;
        }
        Hist { from, to, bars }
    }
}

impl Reader {
    pub fn new() -> Reader {
        Reader::default()
    }

    /// What it comes to in each of `DRAWS` draws. Undefined is NaN, and a truth is 1 or 0. Every column
    /// is over the same draws, so two of them can be compared draw by draw.
    pub fn column(&mut self, r: &Rand) -> Arc<Vec<f64>> {
        if let Some(c) = self.columns.get(&r.0) {
            return c.clone();
        }
        let (parts, names) = {
            let t = arena();
            let parts = t.reach(r.0, |x| self.columns.contains_key(&x));
            let names: HashMap<u32, String> = parts.iter().filter_map(|(_, n)| if let Node::Draw(_, i) = n { Some((*i, t.draws[*i as usize].clone())) } else { None }).collect();
            (parts, names)
        };
        // Only what was asked for is kept: the rest is as much again for every part of it.
        let mut working: HashMap<X, Arc<Vec<f64>>> = HashMap::with_capacity(parts.len());
        for (x, node) in parts {
            let of = |x: &X| working.get(x).or_else(|| self.columns.get(x)).expect("its parts come first").clone();
            let column: Vec<f64> = match &node {
                Node::Num(b) => vec![f64::from_bits(*b); DRAWS],
                Node::Flag(b) => vec![f64::from(u8::from(*b)); DRAWS],
                Node::Draw(std, i) => {
                    let u = self.uniforms.entry(*i).or_insert_with(|| Arc::new(uniforms(&names[i]))).clone();
                    u.iter().map(|u| apply_std(*std, *u)).collect()
                }
                Node::Un(op, a) => of(a).iter().map(|v| apply_un(*op, *v)).collect(),
                Node::Bin(op, a, b) => {
                    let (a, b) = (of(a), of(b));
                    a.iter().zip(b.iter()).map(|(x, y)| apply_bin(*op, *x, *y)).collect()
                }
                Node::Tri(u, a, b, c) => {
                    let (u, a, b, c) = (of(u), of(a), of(b), of(c));
                    (0..DRAWS).map(|i| apply_tri(u[i], a[i], b[i], c[i])).collect()
                }
                Node::If(c, a, b) => {
                    let (c, a, b) = (of(c), of(a), of(b));
                    (0..DRAWS).map(|i| apply_if(c[i], a[i], b[i])).collect()
                }
            };
            working.insert(x, Arc::new(column));
        }
        let column = working.remove(&r.0).expect("just worked out");
        self.columns.insert(r.0, column.clone());
        column
    }

    /// It as a plain number plus so much of each of some draws, if that's what it is.
    fn straight(&mut self, x: X) -> Option<Arc<Straight>> {
        if let Some(known) = self.straight.get(&x) {
            return known.clone();
        }
        let parts = arena().reach(x, |x| self.straight.contains_key(&x));
        for (x, node) in parts {
            let of = |x: &X| self.straight.get(x).cloned().flatten();
            let plain = |s: &Straight| s.of.is_empty().then_some(s.plain);
            let found = match &node {
                Node::Num(b) if !f64::from_bits(*b).is_nan() => Some(Straight { plain: f64::from_bits(*b), of: vec![] }),
                Node::Draw(..) => Some(Straight { plain: 0.0, of: vec![(x, 1.0)] }),
                Node::Un(Un::Neg, a) => of(a).map(|a| a.scaled(-1.0)),
                Node::Bin(Op::Add, a, b) => of(a).zip(of(b)).and_then(|(a, b)| a.plus(&b)),
                Node::Bin(Op::Sub, a, b) => of(a).zip(of(b)).and_then(|(a, b)| a.plus(&b.scaled(-1.0))),
                Node::Bin(Op::Mul, a, b) => of(a).zip(of(b)).and_then(|(a, b)| match (plain(&a), plain(&b)) {
                    (Some(k), _) => Some(b.scaled(k)),
                    (_, Some(k)) => Some(a.scaled(k)),
                    _ => None,
                }),
                Node::Bin(Op::Div, a, b) => of(a).zip(of(b)).and_then(|(a, b)| plain(&b).filter(|k| *k != 0.0).map(|k| a.scaled(1.0 / k))),
                _ => None,
            };
            self.straight.insert(x, found.map(Arc::new));
        }
        self.straight.get(&x).cloned().flatten()
    }

    /// What a straight line over draws is, if every draw is a bell curve, or there's only the one.
    fn law_of_straight(s: &Straight) -> Option<Law> {
        let stds: Vec<Std> = {
            let t = arena();
            s.of.iter().map(|(x, _)| if let Node::Draw(std, _) = t.nodes[*x as usize] { std } else { unreachable!("only draws are kept") }).collect()
        };
        if !stds.is_empty() && stds.iter().all(|s| *s == Std::Normal) {
            // Bell curves that share nothing add to a bell curve, and each draw is here once.
            return Some(Law::Normal { mean: s.plain, sd: s.of.iter().map(|(_, k)| k * k).sum::<f64>().sqrt() });
        }
        match (stds.as_slice(), s.of.as_slice()) {
            ([Std::Uniform], [(_, k)]) => Some(Law::Uniform { from: s.plain.min(s.plain + k), to: s.plain.max(s.plain + k) }),
            ([Std::Exponential], [(_, k)]) if *k > 0.0 => Some(Law::Exponential { from: s.plain, mean: *k }),
            _ => None,
        }
    }

    /// The distribution a number is, if it has a name.
    pub fn law(&mut self, r: &Rand) -> Option<Law> {
        self.law_at(r.0)
    }

    /// How likely a truth is, exactly, where it's a question a law answers: one number against another
    /// where the gap between them has a law, a number with a law against a plain one, two limits on one
    /// such number, or the opposite of any of those.
    fn sure(&mut self, x: X) -> Option<f64> {
        let node = arena().nodes[x as usize].clone();
        match node {
            Node::Un(Un::Not, inner) => Some(1.0 - self.sure(inner)?),
            Node::Bin(Op::Lt | Op::Le, a, b) => {
                if let (Some(left), Some(right)) = (self.straight(a), self.straight(b))
                    && let Some(gap) = left.plus(&right.scaled(-1.0))
                    && let Some(law) = Self::law_of_straight(&gap)
                {
                    return Some(law.below(0.0));
                }
                let plain = {
                    let t = arena();
                    (t.plain(a), t.plain(b))
                };
                match plain {
                    (None, Some(at)) => Some(self.law_at(a)?.below(at)),
                    (Some(at), None) => Some(1.0 - self.law_at(b)?.below(at)),
                    _ => None,
                }
            }
            Node::Bin(op @ (Op::And | Op::Or), a, b) => {
                let (p, q) = {
                    let t = arena();
                    (t.bound(a)?, t.bound(b)?)
                };
                if p.of != q.of {
                    return None;
                }
                let law = self.law_at(p.of)?;
                let holds = |b: &Bound| if b.below { law.below(b.at) } else { 1.0 - law.below(b.at) };
                Some(match (p.below == q.below, op == Op::And) {
                    // On the same side, the tighter is both and the looser is either.
                    (true, true) => holds(&p).min(holds(&q)),
                    (true, false) => holds(&p).max(holds(&q)),
                    (false, both) => {
                        let (under, over) = if p.below { (&p, &q) } else { (&q, &p) };
                        if both { (law.below(under.at) - law.below(over.at)).max(0.0) } else { (law.below(under.at) + 1.0 - law.below(over.at)).min(1.0) }
                    }
                })
            }
            _ => None,
        }
    }

    fn law_at(&mut self, x: X) -> Option<Law> {
        let node = arena().nodes[x as usize].clone();
        match node {
            Node::Un(Un::Exp, inner) => match Self::law_of_straight(&*self.straight(inner)?)? {
                Law::Normal { mean, sd } => Some(Law::Lognormal { mu: mean, sigma: sd }),
                _ => None,
            },
            Node::Tri(u, a, b, c) => {
                let t = arena();
                let Node::Draw(Std::Uniform, _) = t.nodes[u as usize] else { return None };
                match (t.plain(a), t.plain(b), t.plain(c)) {
                    (Some(low), Some(likely), Some(high)) if low <= likely && likely <= high && low < high => Some(Law::Triangular { low, likely, high }),
                    _ => None,
                }
            }
            _ => Self::law_of_straight(&*self.straight(x)?),
        }
    }

    /// One thing a number could be, as values that each count the same, and whether they're exact.
    fn spread(&mut self, value: &Term) -> (Vec<f64>, Option<Law>, bool) {
        match value {
            Term::Undefined => (vec![f64::NAN], None, true),
            Term::Num(n) => (vec![*n], None, true),
            Term::Flag(b) => (vec![f64::from(u8::from(*b))], None, true),
            Term::Rand(r) => match self.law(r) {
                // Drawn from the law itself, evenly, so the picture is the law's and not a sample's.
                Some(law) => ((0..DRAWS).map(|i| law.quantile((i as f64 + 0.5) / DRAWS as f64)).collect(), Some(law), true),
                None => (self.column(r).to_vec(), None, false),
            },
        }
    }

    /// What a number comes to, if it's each of these this much of the time. With one, that's what that
    /// one comes to.
    pub fn summary(&mut self, shares: &[Share]) -> Summary {
        let whole: f64 = shares.iter().map(|s| s.weight).sum();
        let mut values = Vec::new();
        let mut undefined = 0.0;
        let mut exact = true;
        let mut law = None;
        let (mut least, mut most) = (f64::INFINITY, f64::NEG_INFINITY);
        for share in shares.iter().filter(|s| s.weight > 0.0) {
            let (spread, its_law, its_exact) = self.spread(&share.value);
            let each = share.weight / whole / spread.len() as f64;
            undefined += spread.iter().filter(|v| v.is_nan()).count() as f64 * each;
            values.extend(spread.into_iter().map(|v| (v, each)));
            exact &= its_exact;
            law = its_law;
            let (lo, hi) = match &share.value {
                Term::Rand(r) => r.bounds(),
                Term::Num(n) => (*n, *n),
                _ => (f64::INFINITY, f64::NEG_INFINITY),
            };
            (least, most) = (least.min(lo), most.max(hi));
        }
        let several = shares.iter().filter(|s| s.weight > 0.0).count() > 1;
        let values = Weighted::new(values);
        let (mut mean, mut sd, mut low, mut mid, mut high) = (values.mean(), values.sd(), values.quantile(0.05), values.quantile(0.5), values.quantile(0.95));
        // A mixture of laws has no law of its own, so only its mean and spread are exactly known, and
        // it says it was read rather than claim more.
        let law = law.filter(|_| !several);
        if let Some(law) = &law {
            (mean, sd, low, mid, high) = (law.mean(), law.sd(), law.quantile(0.05), law.quantile(0.5), law.quantile(0.95));
        }
        let exact = exact && (law.is_some() || values.0.len() <= shares.len());
        Summary {
            mean,
            sd,
            low,
            mid,
            high,
            least: Some(least).filter(|v| v.is_finite()),
            most: Some(most).filter(|v| v.is_finite()),
            undefined,
            hist: values.hist(),
            law: law.map(|l| l.text()),
            exact,
            draws: if exact { 0 } else { DRAWS },
            error: if exact { 0.0 } else { sd / (DRAWS as f64).sqrt() },
        }
    }

    /// How likely a truth is, if it's each of these this much of the time.
    pub fn chance(&mut self, shares: &[Share]) -> Chance {
        let whole: f64 = shares.iter().map(|s| s.weight).sum();
        let (mut yes, mut undefined, mut exact, mut hits) = (0.0, 0.0, true, 0);
        for share in shares.iter().filter(|s| s.weight > 0.0) {
            let w = share.weight / whole;
            match &share.value {
                Term::Flag(true) => yes += w,
                Term::Rand(r) => {
                    // A law says how likely exactly, and what has one is never undefined.
                    if let Some(sure) = self.sure(r.0) {
                        yes += w * sure;
                        continue;
                    }
                    let column = self.column(r);
                    let nan = column.iter().filter(|v| v.is_nan()).count();
                    let here = column.iter().filter(|v| **v == 1.0).count();
                    yes += w * here as f64 / DRAWS as f64;
                    undefined += w * nan as f64 / DRAWS as f64;
                    hits += here;
                    exact = false;
                }
                Term::Undefined => undefined += w,
                _ => {}
            }
        }
        let chance = if undefined < 1.0 { yes / (1.0 - undefined) } else { 0.0 };
        Chance { chance, undefined, exact, draws: if exact { 0 } else { DRAWS }, hits, error: if exact { 0.0 } else { (chance * (1.0 - chance) / DRAWS as f64).sqrt() } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> Term {
        Term::Num(x)
    }

    fn normal(name: &str, mean: f64, sd: f64) -> Term {
        bin(Op::Add, &n(mean), &bin(Op::Mul, &n(sd), &Term::Rand(draw(Std::Normal, name))))
    }

    fn rand(t: &Term) -> Rand {
        match t {
            Term::Rand(r) => *r,
            other => panic!("{other:?} isn't uncertain"),
        }
    }

    fn one(t: &Term) -> [Share; 1] {
        [Share { weight: 1.0, value: t.clone() }]
    }

    #[test]
    fn probit_is_where_the_bell_curve_says() {
        for (p, z) in [(0.5, 0.0), (0.975, 1.959963984540054), (0.9, 1.2815515655446004), (0.001, -3.090232306167813), (0.02, -2.053748910631823)] {
            assert!((probit(p) - z).abs() < 1e-8, "{p}: {} isn't {z}", probit(p));
        }
    }

    #[test]
    fn the_same_expression_is_the_same_handle() {
        let (a, b) = (normal("t/same/a", 1.0, 2.0), normal("t/same/b", 0.0, 1.0));
        assert_eq!(bin(Op::Add, &a, &b), bin(Op::Add, &b, &a), "adding goes either way round");
        assert_eq!(bin(Op::Sub, &a, &a), n(0.0), "a number less itself is nothing, because it's the same draw");
        assert_ne!(bin(Op::Sub, &a, &normal("t/same/c", 1.0, 2.0)), n(0.0), "and another with the same shape isn't the same draw");
        assert_eq!(bin(Op::Mul, &a, &n(1.0)), a);
        assert_eq!(bin(Op::Add, &n(0.0), &a), a);
    }

    #[test]
    fn what_cant_happen_is_never_and_what_must_is_always() {
        let u = Term::Rand(draw(Std::Uniform, "t/never/u"));
        let spread = bin(Op::Add, &n(2.0), &bin(Op::Mul, &n(2.0), &u));
        assert_eq!(bin(Op::Lt, &spread, &n(5.0)), Term::Flag(true), "between 2 and 4 is always under 5");
        assert_eq!(bin(Op::Lt, &n(4.5), &spread), Term::Flag(false));
        let x = normal("t/never/x", 0.0, 1.0);
        let (over, under) = (bin(Op::Lt, &n(5.0), &x), bin(Op::Lt, &x, &n(3.0)));
        assert_eq!(bin(Op::And, &over, &under), Term::Flag(false), "nothing is over 5 and under 3");
        assert!(matches!(bin(Op::And, &bin(Op::Lt, &n(1.0), &x), &under), Term::Rand(_)), "but something is over 1 and under 3");
        assert!(matches!(bin(Op::Lt, &x, &n(3.0)), Term::Rand(_)), "and whether it's under 3 is a matter of chance");
    }

    #[test]
    fn a_sum_of_bell_curves_is_one_and_knows_what_it_shares() {
        let (a, b) = (normal("t/sum/a", 3.0, 3.0), normal("t/sum/b", 1.0, 4.0));
        let mut reader = Reader::new();
        assert_eq!(reader.law(&rand(&bin(Op::Add, &a, &b))), Some(Law::Normal { mean: 4.0, sd: 5.0 }));
        // Twice one draw is twice as wide, where two draws added are only root two as wide.
        assert_eq!(reader.law(&rand(&bin(Op::Add, &a, &a))), Some(Law::Normal { mean: 6.0, sd: 6.0 }));
        let s = reader.summary(&one(&bin(Op::Add, &a, &b)));
        assert!(s.exact && s.law.as_deref() == Some("normal(4, 5)"), "{s:?}");
        assert_eq!((s.mean, s.sd, s.mid), (4.0, 5.0, 4.0));
    }

    #[test]
    fn a_law_agrees_with_drawing_it() {
        let cases = [
            normal("t/law/n", 2.0, 0.5),
            bin(Op::Add, &n(1.0), &bin(Op::Mul, &n(3.0), &Term::Rand(draw(Std::Uniform, "t/law/u")))),
            bin(Op::Mul, &n(2.0), &Term::Rand(draw(Std::Exponential, "t/law/e"))),
            un(Un::Exp, &normal("t/law/l", 0.5, 0.25)),
            tri(&Term::Rand(draw(Std::Uniform, "t/law/t")), &n(2.0), &n(3.0), &n(6.0)),
        ];
        for case in cases {
            let mut reader = Reader::new();
            let law = reader.law(&rand(&case)).unwrap_or_else(|| panic!("{case:?} has a law"));
            let column = Weighted::new(reader.column(&rand(&case)).iter().map(|v| (*v, 1.0)).collect());
            let near = |a: f64, b: f64| (a - b).abs() < 0.02 * law.sd().max(1e-9) + 1e-9;
            assert!(near(column.mean(), law.mean()), "{law:?}: mean {} isn't {}", column.mean(), law.mean());
            assert!((column.sd() / law.sd() - 1.0).abs() < 0.02, "{law:?}: spread {} isn't {}", column.sd(), law.sd());
            for p in [0.05, 0.5, 0.95] {
                assert!((column.quantile(p) - law.quantile(p)).abs() < 0.03 * law.sd(), "{law:?} at {p}: {} isn't {}", column.quantile(p), law.quantile(p));
            }
        }
    }

    #[test]
    fn what_has_no_law_is_drawn_and_says_so() {
        let (a, b) = (normal("t/drawn/a", 0.0, 1.0), normal("t/drawn/b", 0.0, 1.0));
        let later = bin(Op::Max, &a, &b);
        let mut reader = Reader::new();
        assert_eq!(reader.law(&rand(&later)), None);
        let s = reader.summary(&one(&later));
        assert!(!s.exact && s.draws == DRAWS && s.error > 0.0);
        // The later of two is later than either, on average: 1/√π.
        assert!((s.mean - 0.5641895835).abs() < 3.0 * s.error, "{s:?}");
        let c = reader.chance(&one(&bin(Op::Lt, &later, &n(0.5))));
        // Both under a half: 0.6915 squared.
        assert!(!c.exact && c.draws == DRAWS && (c.chance - 0.478_15).abs() < 3.0 * c.error, "{c:?}");
    }

    #[test]
    fn a_law_is_as_much_below_a_point_as_the_point_it_has_that_much_below() {
        for (x, below) in [(0.0, 0.5), (1.0, 0.841_344_746_068_542_9), (-2.0, 0.022_750_131_948_179_21), (4.0, 0.999_968_328_758_166_9), (-6.0, 9.865_876_450_376_946e-10)] {
            assert!((phi(x) / below - 1.0).abs() < 1e-12, "{x}: {} isn't {below}", phi(x));
        }
        assert!(phi(-40.0) > 0.0 && phi(40.0) < 1.0, "nothing's impossible for a bell curve");
        let laws = [
            Law::Normal { mean: 2.0, sd: 0.5 },
            Law::Uniform { from: 1.0, to: 4.0 },
            Law::Exponential { from: 1.0, mean: 2.0 },
            Law::Lognormal { mu: 0.5, sigma: 0.25 },
            Law::Triangular { low: 2.0, likely: 3.0, high: 6.0 },
        ];
        for law in laws {
            for p in [0.001, 0.05, 0.3, 0.5, 0.9, 0.999] {
                assert!((law.below(law.quantile(p)) - p).abs() < 1e-8, "{law:?} at {p}: {}", law.below(law.quantile(p)));
            }
        }
    }

    #[test]
    fn a_question_a_law_answers_is_answered_exactly() {
        let (a, b) = (normal("t/sure/a", 3.0, 3.0), normal("t/sure/b", 1.0, 4.0));
        let mut reader = Reader::new();
        let mut sure = |t: Term| {
            let c = reader.chance(&one(&t));
            assert!(c.exact && c.draws == 0 && c.error == 0.0 && c.undefined == 0.0, "{c:?}");
            c.chance
        };
        let near = |got: f64, want: f64| assert!((got - want).abs() < 1e-12, "{got} isn't {want}");
        // The gap between two unrelated bell curves is one: 2 give or take 5.
        near(sure(bin(Op::Lt, &a, &b)), phi(-2.0 / 5.0));
        near(sure(bin(Op::Lt, &a, &n(6.0))), phi(1.0));
        near(sure(bin(Op::Lt, &n(6.0), &a)), 1.0 - phi(1.0));
        near(sure(un(Un::Not, &bin(Op::Le, &a, &n(6.0)))), 1.0 - phi(1.0));
        // Between two points, and outside them.
        let (over, under) = (bin(Op::Lt, &n(0.0), &a), bin(Op::Lt, &a, &n(6.0)));
        near(sure(bin(Op::And, &over, &under)), phi(1.0) - phi(-1.0));
        near(sure(bin(Op::Or, &bin(Op::Lt, &a, &n(0.0)), &bin(Op::Lt, &n(6.0), &a))), 1.0 - (phi(1.0) - phi(-1.0)));
        // A law that isn't a straight line of draws answers against a plain number too.
        let estimate = tri(&Term::Rand(draw(Std::Uniform, "t/sure/t")), &n(2.0), &n(3.0), &n(6.0));
        near(sure(bin(Op::Lt, &estimate, &n(3.0))), 0.25);
        // And what covers everything is always, with nothing to read.
        assert_eq!(bin(Op::Or, &bin(Op::Lt, &n(1.0), &a), &bin(Op::Le, &a, &n(1.0))), Term::Flag(true));
    }

    #[test]
    fn drawing_is_the_same_every_time() {
        let x = bin(Op::Max, &normal("t/again/a", 0.0, 1.0), &normal("t/again/b", 0.0, 1.0));
        let (first, second) = (Reader::new().summary(&one(&x)), Reader::new().summary(&one(&x)));
        assert_eq!(first, second);
    }

    #[test]
    fn a_mixture_weighs_each_by_how_often() {
        let mut reader = Reader::new();
        let s = reader.summary(&[Share { weight: 3.0, value: n(10.0) }, Share { weight: 1.0, value: n(20.0) }]);
        assert!(s.exact && s.mean == 12.5 && s.mid == 10.0, "{s:?}");
        let s = reader.summary(&[Share { weight: 1.0, value: normal("t/mix/a", 0.0, 1.0) }, Share { weight: 1.0, value: normal("t/mix/b", 10.0, 1.0) }]);
        assert!(s.law.is_none() && !s.exact && (s.mean - 5.0).abs() < 0.05, "{s:?}");
    }

    #[test]
    fn it_survives_being_written_out() {
        let x = bin(Op::Max, &normal("t/json/a", 1.0, 2.0), &un(Un::Exp, &normal("t/json/b", 0.0, 1.0)));
        let json = serde_json::to_value(rand(&x)).unwrap();
        let back: Rand = serde_json::from_value(json).unwrap();
        assert_eq!(back, rand(&x));
        assert!(serde_json::from_value::<Rand>(serde_json::json!({ "rand": 5 })).is_err());
        assert!(serde_json::from_value::<Rand>(serde_json::json!({ "other": 5 })).is_err());
    }

    #[test]
    fn its_value_in_one_draw_is_what_plain_arithmetic_gives() {
        let (a, b) = (normal("t/at/a", 1.0, 2.0), Term::Rand(draw(Std::Uniform, "t/at/b")));
        let x = rand(&pick(&bin(Op::Lt, &b, &n(0.5)), &bin(Op::Mul, &a, &b), &un(Un::Ln, &b)));
        let at = |ua: f64, ub: f64| x.at(&|name| if name == "t/at/a" { ua } else { ub });
        assert_eq!(at(0.5, 0.25), Some((1.0 + 2.0 * probit(0.5)) * 0.25));
        assert_eq!(at(0.5, 0.75), Some(0.75f64.ln()));
    }
}
