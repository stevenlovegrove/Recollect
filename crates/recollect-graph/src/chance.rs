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
            // Rounding what's already whole changes nothing.
            (Un::Floor | Un::Ceil, Node::Un(Un::Floor | Un::Ceil, _)) => return a,
            // Not below is at or above, where both sides are always something.
            (Un::Not, Node::Bin(cmp @ (Op::Lt | Op::Le), l, r)) if self.total(a) => {
                let (cmp, l, r) = (*cmp, *l, *r);
                return self.bin(if cmp == Op::Lt { Op::Le } else { Op::Lt }, r, l);
            }
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
            Op::Sub if self.is(a, 0.0) => return self.un(Un::Neg, b),
            // A number over itself is one, where it's never nothing.
            Op::Div if a == b && both && (al > 0.0 || ah < 0.0) => return self.num(1.0),
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
        // Something and its opposite: never both, and always one or the other.
        if matches!(op, Op::And | Op::Or) && both {
            let opposite = |x: X, y: X| self.nodes[x as usize] == Node::Un(Un::Not, y);
            if opposite(a, b) || opposite(b, a) {
                return self.flag(op == Op::Or);
            }
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
        // Choosing on the opposite is choosing the other way round, and at or below is the opposite of
        // above where both sides are always something. So each choice is held one way.
        match self.nodes[c as usize] {
            Node::Un(Un::Not, inner) => return self.pick(inner, b, a),
            Node::Bin(Op::Le, l, r) if self.total(c) => {
                let above = self.bin(Op::Lt, r, l);
                return self.pick(above, b, a);
            }
            _ => {}
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

// ---------- laws: what an expression is exactly, where that can be said ----------

/// A distribution that can be read exactly: one with a name, or one made of those in a way that still
/// says how much of it is below any point.
#[derive(Debug, Clone, PartialEq)]
pub enum Law {
    Normal { mean: f64, sd: f64 },
    Uniform { from: f64, to: f64 },
    /// A wait that starts at `from` and averages `mean` more.
    Exponential { from: f64, mean: f64 },
    /// A number whose log is a normal with this mean and spread.
    Lognormal { mu: f64, sigma: f64 },
    Triangular { low: f64, likely: f64, high: f64 },
    /// One number, for certain.
    Point(f64),
    /// The greatest of several that share no draw.
    Greatest(Vec<Law>),
    /// The least of several that share no draw.
    Least(Vec<Law>),
    /// Each of these this much of the time. The shares add to 1.
    Mixed(Vec<(f64, Law)>),
    /// One of the smooth ones above, times `by` and then plus `shift`, where that has no name of its
    /// own: a wait turned over, or a lognormal moved along.
    Moved { of: Box<Law>, by: f64, shift: f64 },
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

/// Where a sum is worked out between two points, and how much each counts: five-point Gauss-Legendre.
const GAUSS: [(f64, f64); 5] = [
    (0.0, 0.568_888_888_888_888_9),
    (0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (-0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (0.906_179_845_938_664, 0.236_926_885_056_189_1),
    (-0.906_179_845_938_664, 0.236_926_885_056_189_1),
];

impl Law {
    /// How much of its weight is below this point, or at it too if `inclusive`. The two differ only where
    /// some of the weight is on one number, as `max(x, 0)` has at 0.
    pub fn mass(&self, x: f64, inclusive: bool) -> f64 {
        match self {
            Law::Normal { mean, sd } => phi((x - mean) / sd),
            Law::Uniform { from, to } => ((x - from) / (to - from)).clamp(0.0, 1.0),
            Law::Exponential { from, .. } if x <= *from => 0.0,
            // A wait is never certainly over, however long it's been.
            Law::Exponential { from, mean } => (-(-(x - from) / mean).exp_m1()).min(1.0 - f64::EPSILON / 2.0),
            Law::Lognormal { .. } if x <= 0.0 => 0.0,
            Law::Lognormal { mu, sigma } => phi((x.ln() - mu) / sigma),
            Law::Triangular { low, .. } if x <= *low => 0.0,
            Law::Triangular { high, .. } if x >= *high => 1.0,
            Law::Triangular { low, likely, high } if x <= *likely => (x - low) * (x - low) / ((high - low) * (likely - low)),
            Law::Triangular { low, likely, high } => 1.0 - (high - x) * (high - x) / ((high - low) * (high - likely)),
            Law::Point(at) => f64::from(u8::from(x > *at || (inclusive && x == *at))),
            // All of them are below a point exactly when the greatest is, and they share nothing.
            Law::Greatest(of) => of.iter().map(|l| l.mass(x, inclusive)).product(),
            Law::Least(of) => 1.0 - of.iter().map(|l| 1.0 - l.mass(x, inclusive)).product::<f64>(),
            Law::Mixed(of) => of.iter().map(|(share, l)| share * l.mass(x, inclusive)).sum(),
            Law::Moved { of, by, shift } if *by > 0.0 => of.mass((x - shift) / by, inclusive),
            // Turned over, what was above is below.
            Law::Moved { of, by, shift } => 1.0 - of.mass((x - shift) / by, inclusive),
        }
    }

    /// How much of its weight is at or below this.
    pub fn below(&self, x: f64) -> f64 {
        self.mass(x, true)
    }

    /// The numbers some of its weight sits on, rather than being spread about.
    fn points(&self, out: &mut Vec<f64>) {
        match self {
            Law::Point(at) => out.push(*at),
            Law::Greatest(of) | Law::Least(of) => of.iter().for_each(|l| l.points(out)),
            Law::Mixed(of) => of.iter().for_each(|(_, l)| l.points(out)),
            _ => {}
        }
    }

    /// Where it has this much of its weight at or below it.
    pub fn quantile(&self, p: f64) -> f64 {
        let (low, high) = match self {
            Law::Normal { mean, sd } => return mean + sd * probit(p),
            Law::Uniform { from, to } => return from + (to - from) * p,
            Law::Exponential { from, mean } => return from + mean * apply_std(Std::Exponential, p),
            Law::Lognormal { mu, sigma } => return (mu + sigma * probit(p)).exp(),
            Law::Triangular { low, likely, high } => return apply_tri(p, *low, *likely, *high),
            Law::Point(at) => return *at,
            Law::Moved { of, by, shift } => return shift + by * of.quantile(if *by > 0.0 { p } else { 1.0 - p }),
            // Between where each of them has this much, and where each has enough that together they do.
            Law::Greatest(of) => {
                let each = p.powf(1.0 / of.len() as f64);
                (of.iter().map(|l| l.quantile(p)).fold(f64::NEG_INFINITY, f64::max), of.iter().map(|l| l.quantile(each)).fold(f64::NEG_INFINITY, f64::max))
            }
            Law::Least(of) => {
                let each = 1.0 - (1.0 - p).powf(1.0 / of.len() as f64);
                (of.iter().map(|l| l.quantile(each)).fold(f64::INFINITY, f64::min), of.iter().map(|l| l.quantile(p)).fold(f64::INFINITY, f64::min))
            }
            Law::Mixed(of) => (of.iter().map(|(_, l)| l.quantile(p)).fold(f64::INFINITY, f64::min), of.iter().map(|(_, l)| l.quantile(p)).fold(f64::NEG_INFINITY, f64::max)),
        };
        // A number with weight on it is the answer for every share that weight spans, and exactly so.
        let mut points = Vec::new();
        self.points(&mut points);
        if let Some(at) = points.into_iter().find(|at| self.mass(*at, true) >= p && self.mass(*at, false) < p) {
            return at;
        }
        let (mut low, mut high) = (low, high);
        for _ in 0..200 {
            let mid = low + (high - low) / 2.0;
            if !(mid > low && mid < high) {
                break;
            }
            if self.mass(mid, true) >= p {
                high = mid;
            } else {
                low = mid;
            }
        }
        high
    }

    /// Its mean and its spread, for one with no formula for them: summed between a few hundred points
    /// chosen so each stretch holds little of the weight, and finely out in the tails. It's right to about
    /// nine figures where the law is smooth.
    fn moments(&self) -> (f64, f64) {
        let tails = || (9..=50).map(|k| 0.5f64.powi(k));
        let mut shares: Vec<f64> = tails().collect();
        shares.reverse();
        shares.extend((1..256).map(|i| f64::from(i) / 256.0));
        shares.extend(tails().map(|p| 1.0 - p));
        let at: Vec<f64> = shares.iter().map(|p| self.quantile(*p)).collect();
        let (first, last) = (at[0], at[at.len() - 1]);
        // What's beyond the last point either way counts as being at it.
        let (under, over) = (self.below(first), 1.0 - self.below(last));
        let (mut mean, mut square) = (first * under + last * over, first * first * under + last * last * over);
        for pair in at.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if !(b > a) {
                continue;
            }
            let (half, middle) = ((b - a) / 2.0, (a + b) / 2.0);
            let (mut area, mut moment) = (0.0, 0.0);
            for (x, weight) in GAUSS {
                let x = middle + half * x;
                let below = self.below(x);
                area += weight * below * half;
                moment += weight * x * below * half;
            }
            // By parts: the sum of x over the weight between a and b is what's below b times b, less the
            // same at a, less the area under how much is below.
            let (fa, fb) = (self.below(a), self.below(b));
            mean += b * fb - a * fa - area;
            square += b * b * fb - a * a * fa - 2.0 * moment;
        }
        (mean, (square - mean * mean).max(0.0).sqrt())
    }

    pub fn mean(&self) -> f64 {
        match self {
            Law::Normal { mean, .. } => *mean,
            Law::Uniform { from, to } => (from + to) / 2.0,
            Law::Exponential { from, mean } => from + mean,
            Law::Lognormal { mu, sigma } => (mu + sigma * sigma / 2.0).exp(),
            Law::Triangular { low, likely, high } => (low + likely + high) / 3.0,
            Law::Point(at) => *at,
            Law::Greatest(_) | Law::Least(_) => self.moments().0,
            Law::Mixed(of) => of.iter().map(|(share, l)| share * l.mean()).sum(),
            Law::Moved { of, by, shift } => shift + by * of.mean(),
        }
    }

    pub fn sd(&self) -> f64 {
        match self {
            Law::Normal { sd, .. } => *sd,
            Law::Uniform { from, to } => (to - from) / 12f64.sqrt(),
            Law::Exponential { mean, .. } => *mean,
            Law::Lognormal { mu, sigma } => (((sigma * sigma).exp() - 1.0) * (2.0 * mu + sigma * sigma).exp()).sqrt(),
            Law::Triangular { low: a, likely: b, high: c } => ((a * a + b * b + c * c - a * b - a * c - b * c) / 18.0).sqrt(),
            Law::Point(_) => 0.0,
            Law::Greatest(_) | Law::Least(_) => self.moments().1,
            Law::Moved { of, by, .. } => by.abs() * of.sd(),
            Law::Mixed(of) => {
                let mean = self.mean();
                let square: f64 = of
                    .iter()
                    .map(|(share, l)| {
                        let (m, s) = (l.mean(), l.sd());
                        share * (s * s + m * m)
                    })
                    .sum();
                (square - mean * mean).max(0.0).sqrt()
            }
        }
    }

    /// This, times `by` and then plus `shift`. Only nothing times it, or something that isn't a number,
    /// leaves nothing to say.
    pub fn scaled(&self, by: f64, shift: f64) -> Option<Law> {
        if by == 0.0 || !by.is_finite() || !shift.is_finite() {
            return None;
        }
        let at = |x: f64| x * by + shift;
        let each = |of: &[Law]| of.iter().map(|l| l.scaled(by, shift)).collect::<Option<Vec<Law>>>();
        Some(match self {
            Law::Normal { mean, sd } => Law::Normal { mean: at(*mean), sd: sd * by.abs() },
            Law::Uniform { from, to } => Law::Uniform { from: at(*from).min(at(*to)), to: at(*from).max(at(*to)) },
            Law::Exponential { from, mean } if by > 0.0 => Law::Exponential { from: at(*from), mean: mean * by },
            Law::Lognormal { mu, sigma } if by > 0.0 && shift == 0.0 => Law::Lognormal { mu: mu + by.ln(), sigma: *sigma },
            Law::Triangular { low, likely, high } if by > 0.0 => Law::Triangular { low: at(*low), likely: at(*likely), high: at(*high) },
            Law::Triangular { low, likely, high } => Law::Triangular { low: at(*high), likely: at(*likely), high: at(*low) },
            Law::Point(x) => Law::Point(at(*x)),
            // Turned over, the greatest is the least.
            Law::Greatest(of) if by > 0.0 => Law::Greatest(each(of)?),
            Law::Greatest(of) => Law::Least(each(of)?),
            Law::Least(of) if by > 0.0 => Law::Least(each(of)?),
            Law::Least(of) => Law::Greatest(each(of)?),
            Law::Mixed(of) => Law::Mixed(of.iter().map(|(share, l)| Some((*share, l.scaled(by, shift)?))).collect::<Option<_>>()?),
            Law::Moved { of, by: was, shift: from } => return of.scaled(was * by, at(*from)),
            Law::Exponential { .. } | Law::Lognormal { .. } => Law::Moved { of: Box::new(self.clone()), by, shift },
        })
    }

    /// As it would be written in a formula, if it can be.
    pub fn text(&self) -> Option<String> {
        let listed = |name: &str, of: &[Law]| Some(format!("{name}({})", of.iter().map(Law::text).collect::<Option<Vec<String>>>()?.join(", ")));
        match self {
            Law::Normal { mean, sd } => Some(format!("normal({}, {})", figures(*mean), figures(*sd))),
            Law::Uniform { from, to } => Some(format!("uniform({}, {})", figures(*from), figures(*to))),
            Law::Exponential { from, mean } if *from == 0.0 => Some(format!("exponential({})", figures(*mean))),
            Law::Exponential { from, mean } => Some(format!("{} + exponential({})", figures(*from), figures(*mean))),
            Law::Lognormal { mu, sigma } => Some(format!("lognormal({}, {})", figures(*mu), figures(*sigma))),
            Law::Triangular { low, likely, high } => Some(format!("triangular({}, {}, {})", figures(*low), figures(*likely), figures(*high))),
            Law::Point(at) => Some(figures(*at)),
            Law::Greatest(of) => listed("max", of),
            Law::Least(of) => listed("min", of),
            Law::Mixed(_) => None,
            Law::Moved { of, by, shift } => {
                let of = of.text()?;
                let stretched = if *by == 1.0 { of } else if *by == -1.0 { format!("-{of}") } else { format!("{} * {of}", figures(*by)) };
                Some(if *shift == 0.0 { stretched } else if let Some(taken) = stretched.strip_prefix('-') { format!("{} - {taken}", figures(*shift)) } else { format!("{} + {stretched}", figures(*shift)) })
            }
        }
    }

    /// Its histogram: how much is in each stretch, from what's below each end.
    fn hist(&self) -> Hist {
        let (from, to) = (self.quantile(0.002), self.quantile(0.998));
        if !(to > from) {
            return Hist { from, to, bars: vec![1.0] };
        }
        let edge = |i: usize| from + (to - from) * i as f64 / BARS as f64;
        let bars = (0..BARS)
            .map(|i| {
                // What's beyond either end is in the first or the last.
                let under = if i == 0 { 0.0 } else { self.below(edge(i)) };
                let up_to = if i == BARS - 1 { 1.0 } else { self.below(edge(i + 1)) };
                (up_to - under).max(0.0)
            })
            .collect();
        Hist { from, to, bars }
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
        // None of a draw is no draw at all, and what's left has to say so.
        Straight { plain: self.plain * by, of: self.of.iter().map(|(x, k)| (*x, k * by)).filter(|(_, k)| *k != 0.0).collect() }
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

    /// The plain number this is, if no draw is left in it.
    fn only(&self) -> Option<f64> {
        self.of.is_empty().then_some(self.plain)
    }
}

/// What an expression is as a straight line over draws, and what its log is as one. The second is how
/// products and powers of what's above nothing keep a law, as sums do.
#[derive(Clone, Default)]
struct Forms {
    straight: Option<Arc<Straight>>,
    log: Option<Arc<Straight>>,
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
    /// How much of the time it's below 0, where its histogram crosses 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below0: Option<f64>,
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
    /// In how many of the draws it was true. Where it's several things, each drawn, that's of all their draws.
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
    forms: HashMap<X, Forms>,
    laws: HashMap<X, Option<Law>>,
    draws: HashMap<X, Arc<std::collections::BTreeSet<u32>>>,
    /// Something known to be true, which every reading is given: only the draws where it holds count.
    known: Option<Rand>,
    kept: Option<Arc<Vec<bool>>>,
}

/// How far a law is looked for through one thing made of another. Past this it's read from draws, which
/// is always right, where looking further could run out of room.
const MOST_DEEP: usize = 48;

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

    /// A reader for whom something is known: an uncertain truth that's taken to hold. What shares a
    /// draw with it is read only from the draws where it does, and says how many those were. What shares
    /// none is as it was, so a law it has still stands.
    pub fn given(known: Option<Rand>) -> Reader {
        Reader { known, ..Reader::default() }
    }

    /// Whether what's known says anything about this: they share a draw.
    fn tied(&mut self, r: &Rand) -> bool {
        match self.known {
            Some(known) => !self.apart(known.0, r.0),
            None => false,
        }
    }

    /// The draws where what's known holds.
    fn kept(&mut self) -> Arc<Vec<bool>> {
        if let Some(kept) = &self.kept {
            return kept.clone();
        }
        let kept = Arc::new(match self.known {
            Some(known) => self.column(&known).iter().map(|v| *v == 1.0).collect(),
            None => vec![true; DRAWS],
        });
        self.kept = Some(kept.clone());
        kept
    }

    /// In how many of the draws what's known holds: all of them, if nothing is.
    pub fn draws_kept(&mut self) -> usize {
        self.kept().iter().filter(|k| **k).count()
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

    /// It as a straight line over draws, and its log as one, where each is that.
    fn forms(&mut self, x: X) -> Forms {
        if let Some(known) = self.forms.get(&x) {
            return known.clone();
        }
        let parts = arena().reach(x, |x| self.forms.contains_key(&x));
        for (x, node) in parts {
            let of = |x: &X| self.forms.get(x).cloned().unwrap_or_default();
            let both = |a: &X, b: &X, f: &dyn Fn(&Straight, &Straight) -> Option<Straight>, log: bool| {
                let (a, b) = (of(a), of(b));
                let (a, b) = if log { (a.log, b.log) } else { (a.straight, b.straight) };
                a.zip(b).and_then(|(a, b)| f(&a, &b))
            };
            let add = |a: &Straight, b: &Straight| a.plus(b);
            let take = |a: &Straight, b: &Straight| a.plus(&b.scaled(-1.0));
            let straight = match &node {
                Node::Num(b) if !f64::from_bits(*b).is_nan() => Some(Straight { plain: f64::from_bits(*b), of: vec![] }),
                Node::Draw(..) => Some(Straight { plain: 0.0, of: vec![(x, 1.0)] }),
                Node::Un(Un::Neg, a) => of(a).straight.map(|a| a.scaled(-1.0)),
                // The log of what's a product of powers is a sum.
                Node::Un(Un::Ln, a) => of(a).log.map(|a| (*a).clone()),
                Node::Bin(Op::Add, a, b) => both(a, b, &add, false),
                Node::Bin(Op::Sub, a, b) => both(a, b, &take, false),
                Node::Bin(Op::Mul, a, b) => both(a, b, &|a, b| a.only().map(|k| b.scaled(k)).or_else(|| b.only().map(|k| a.scaled(k))), false),
                Node::Bin(Op::Div, a, b) => both(a, b, &|a, b| b.only().filter(|k| *k != 0.0).map(|k| a.scaled(1.0 / k)), false),
                _ => None,
            };
            let log = match &node {
                Node::Num(b) if f64::from_bits(*b) > 0.0 => Some(Straight { plain: f64::from_bits(*b).ln(), of: vec![] }),
                Node::Un(Un::Exp, a) => of(a).straight.map(|a| (*a).clone()),
                Node::Un(Un::Sqrt, a) => of(a).log.map(|a| a.scaled(0.5)),
                Node::Bin(Op::Mul, a, b) => both(a, b, &add, true),
                Node::Bin(Op::Div, a, b) => both(a, b, &take, true),
                Node::Bin(Op::Pow, a, b) => of(a).log.zip(of(b).straight.and_then(|k| k.only())).map(|(a, k)| a.scaled(k)),
                _ => None,
            };
            self.forms.insert(x, Forms { straight: straight.map(Arc::new), log: log.map(Arc::new) });
        }
        self.forms.get(&x).cloned().unwrap_or_default()
    }

    fn straight(&mut self, x: X) -> Option<Arc<Straight>> {
        self.forms(x).straight
    }

    /// The draws it depends on. Two things that share none have nothing to do with each other.
    fn draws_of(&mut self, x: X) -> Arc<std::collections::BTreeSet<u32>> {
        if let Some(known) = self.draws.get(&x) {
            return known.clone();
        }
        let found: std::collections::BTreeSet<u32> = arena().reach(x, |_| false).into_iter().filter_map(|(_, n)| if let Node::Draw(_, i) = n { Some(i) } else { None }).collect();
        let found = Arc::new(found);
        self.draws.insert(x, found.clone());
        found
    }

    fn apart(&mut self, a: X, b: X) -> bool {
        let (a, b) = (self.draws_of(a), self.draws_of(b));
        a.is_disjoint(&b)
    }

    /// What a straight line over draws is, if every draw is a bell curve, or there's only the one.
    fn law_of_straight(s: &Straight) -> Option<Law> {
        let stds: Vec<Std> = {
            let t = arena();
            s.of.iter().map(|(x, _)| if let Node::Draw(std, _) = t.nodes[*x as usize] { std } else { unreachable!("only draws are kept") }).collect()
        };
        if !stds.is_empty() && stds.iter().all(|s| *s == Std::Normal) {
            // Bell curves that share nothing add to a bell curve, and each draw is here once.
            let sd = s.of.iter().map(|(_, k)| k * k).sum::<f64>().sqrt();
            return (sd > 0.0 && sd.is_finite()).then_some(Law::Normal { mean: s.plain, sd });
        }
        match (stds.as_slice(), s.of.as_slice()) {
            ([Std::Uniform], [(_, k)]) if *k != 0.0 => Some(Law::Uniform { from: s.plain.min(s.plain + k), to: s.plain.max(s.plain + k) }),
            ([Std::Exponential], [(_, k)]) if *k > 0.0 => Some(Law::Exponential { from: s.plain, mean: *k }),
            _ => None,
        }
    }

    /// The distribution a number is, if that can be said exactly.
    pub fn law(&mut self, r: &Rand) -> Option<Law> {
        self.law_at(r.0, 0)
    }

    /// How likely a truth is, exactly, where it's a question a law answers: one number against another
    /// where the gap between them has a law, a number with a law against a plain one, two limits on one
    /// such number, two questions about things that share nothing, or the opposite of any of those.
    fn sure(&mut self, x: X, depth: usize) -> Option<f64> {
        if depth > MOST_DEEP {
            return None;
        }
        let node = arena().nodes[x as usize].clone();
        let plain = |x: X| arena().plain(x).filter(|v| !v.is_nan());
        match node {
            Node::Un(Un::Not, inner) => Some(1.0 - self.sure(inner, depth + 1)?),
            Node::Bin(op @ (Op::Lt | Op::Le | Op::Eq | Op::Ne), a, b) => {
                let compares = matches!(op, Op::Lt | Op::Le);
                if let (Some(left), Some(right)) = (self.straight(a), self.straight(b))
                    && let Some(gap) = left.plus(&right.scaled(-1.0))
                {
                    // With no draw left in it, the gap is one number, whatever the draws are.
                    if let Some(gap) = gap.only() {
                        let holds = match op {
                            Op::Lt => gap < 0.0,
                            Op::Le => gap <= 0.0,
                            Op::Eq => gap == 0.0,
                            _ => gap != 0.0,
                        };
                        return Some(f64::from(u8::from(holds)));
                    }
                    // Any law a straight line has is smooth, so it's never exactly anything.
                    if let Some(law) = Self::law_of_straight(&gap) {
                        return Some(match op {
                            Op::Lt | Op::Le => law.below(0.0),
                            Op::Eq => 0.0,
                            _ => 1.0,
                        });
                    }
                }
                // Two products of powers are in the order their logs are, and the gap between those is a sum.
                if let (Some(left), Some(right)) = (self.forms(a).log, self.forms(b).log)
                    && let Some(gap) = left.plus(&right.scaled(-1.0))
                    && let Some(law) = Self::law_of_straight(&gap)
                {
                    return Some(match op {
                        Op::Lt | Op::Le => law.below(0.0),
                        Op::Eq => 0.0,
                        _ => 1.0,
                    });
                }
                match (plain(a), plain(b)) {
                    (None, Some(at)) if compares => Some(self.law_at(a, 0)?.mass(at, op == Op::Le)),
                    (Some(at), None) if compares => Some(1.0 - self.law_at(b, 0)?.mass(at, op == Op::Lt)),
                    (None, Some(at)) | (Some(at), None) => {
                        let law = self.law_at(if plain(a).is_some() { b } else { a }, 0)?;
                        let on = law.mass(at, true) - law.mass(at, false);
                        Some(if op == Op::Eq { on } else { 1.0 - on })
                    }
                    _ => None,
                }
            }
            Node::Bin(op @ (Op::And | Op::Or), a, b) => {
                let limits = {
                    let t = arena();
                    t.bound(a).zip(t.bound(b)).filter(|(p, q)| p.of == q.of)
                };
                if let Some((p, q)) = limits
                    && let Some(law) = self.law_at(p.of, 0)
                {
                    let holds = |b: &Bound| if b.below { law.mass(b.at, !b.strict) } else { 1.0 - law.mass(b.at, b.strict) };
                    return Some(match (p.below == q.below, op == Op::And) {
                        // On the same side, the tighter is both and the looser is either.
                        (true, true) => holds(&p).min(holds(&q)),
                        (true, false) => holds(&p).max(holds(&q)),
                        (false, both) => {
                            let (under, over) = if p.below { (&p, &q) } else { (&q, &p) };
                            let (up_to, from) = (law.mass(under.at, !under.strict), law.mass(over.at, over.strict));
                            if both { (up_to - from).max(0.0) } else { (up_to + 1.0 - from).min(1.0) }
                        }
                    });
                }
                // Two questions about things that share nothing: both is one times the other.
                if !self.apart(a, b) {
                    return None;
                }
                let (p, q) = (self.sure(a, depth + 1)?, self.sure(b, depth + 1)?);
                Some(if op == Op::And { p * q } else { 1.0 - (1.0 - p) * (1.0 - q) })
            }
            _ => None,
        }
    }

    fn law_at(&mut self, x: X, depth: usize) -> Option<Law> {
        if let Some(known) = self.laws.get(&x) {
            return known.clone();
        }
        let found = self.find_law(x, depth);
        self.laws.insert(x, found.clone());
        found
    }

    fn find_law(&mut self, x: X, depth: usize) -> Option<Law> {
        if depth > MOST_DEEP {
            return None;
        }
        let forms = self.forms(x);
        if let Some(law) = forms.straight.as_deref().and_then(Self::law_of_straight) {
            return Some(law);
        }
        // A product of powers of things whose logs are bell curves has a log that's one.
        if let Some(Law::Normal { mean, sd }) = forms.log.as_deref().and_then(Self::law_of_straight) {
            return Some(Law::Lognormal { mu: mean, sigma: sd });
        }
        let node = arena().nodes[x as usize].clone();
        let plain = |x: X| arena().plain(x).filter(|v| !v.is_nan());
        let next = depth + 1;
        match node {
            Node::Tri(u, a, b, c) => {
                let t = arena();
                let Node::Draw(Std::Uniform, _) = t.nodes[u as usize] else { return None };
                match (t.plain(a), t.plain(b), t.plain(c)) {
                    (Some(low), Some(likely), Some(high)) if low <= likely && likely <= high && low < high => Some(Law::Triangular { low, likely, high }),
                    _ => None,
                }
            }
            // Something with a law, moved or stretched by a plain number, still has one.
            Node::Un(Un::Neg, a) => self.law_at(a, next)?.scaled(-1.0, 0.0),
            Node::Bin(Op::Add, a, b) => match (plain(a), plain(b)) {
                (Some(c), None) => self.law_at(b, next)?.scaled(1.0, c),
                (None, Some(c)) => self.law_at(a, next)?.scaled(1.0, c),
                _ => None,
            },
            Node::Bin(Op::Sub, a, b) => match (plain(a), plain(b)) {
                (Some(c), None) => self.law_at(b, next)?.scaled(-1.0, c),
                (None, Some(c)) => self.law_at(a, next)?.scaled(1.0, -c),
                _ => None,
            },
            Node::Bin(Op::Mul, a, b) => match (plain(a), plain(b)) {
                (Some(k), None) => self.law_at(b, next)?.scaled(k, 0.0),
                (None, Some(k)) => self.law_at(a, next)?.scaled(k, 0.0),
                _ => None,
            },
            Node::Bin(Op::Div, a, b) => self.law_at(a, next)?.scaled(1.0 / plain(b)?, 0.0),
            // The later, or the earlier, of things that share nothing: all of them are below a point
            // exactly when the later is.
            Node::Bin(op @ (Op::Max | Op::Min), ..) => {
                let mut among = Vec::new();
                {
                    let t = arena();
                    let mut stack = vec![x];
                    while let Some(y) = stack.pop() {
                        match t.nodes[y as usize] {
                            Node::Bin(o, a, b) if o == op => stack.extend([b, a]),
                            _ => among.push(y),
                        }
                    }
                }
                let mut seen = std::collections::BTreeSet::new();
                let mut laws = Vec::with_capacity(among.len());
                for y in among {
                    if self.draws_of(y).iter().any(|d| !seen.insert(*d)) {
                        return None;
                    }
                    laws.push(match plain(y) {
                        Some(at) => Law::Point(at),
                        None => self.law_at(y, next)?,
                    });
                }
                // In an order of their own, so it reads the same whichever was made first: what's spread
                // about before any plain number, and each as it's written.
                // Of several plain numbers only the greatest, or the least, has any say.
                let plains: Vec<f64> = laws.iter().filter_map(|l| if let Law::Point(at) = l { Some(*at) } else { None }).collect();
                if plains.len() > 1 {
                    let kept = plains.iter().copied().fold(if op == Op::Max { f64::NEG_INFINITY } else { f64::INFINITY }, if op == Op::Max { f64::max } else { f64::min });
                    laws.retain(|l| !matches!(l, Law::Point(_)));
                    laws.push(Law::Point(kept));
                }
                laws.sort_by_cached_key(|l| (matches!(l, Law::Point(_)), l.text().unwrap_or_default()));
                Some(if op == Op::Max { Law::Greatest(laws) } else { Law::Least(laws) })
            }
            _ => None,
        }
    }

    /// One thing a number could be, as values that each count the same.
    fn spread(&mut self, value: &Term) -> Vec<f64> {
        match value {
            Term::Undefined => vec![f64::NAN],
            Term::Num(n) => vec![*n],
            Term::Flag(b) => vec![f64::from(u8::from(*b))],
            // What's known says something about this one, so it's only the draws where that holds.
            Term::Rand(r) if self.tied(r) => {
                let (column, kept) = (self.column(r), self.kept());
                let left: Vec<f64> = column.iter().zip(kept.iter()).filter(|(_, k)| **k).map(|(v, _)| *v).collect();
                if left.is_empty() { vec![f64::NAN] } else { left }
            }
            Term::Rand(r) => match self.law(r) {
                // From the law itself, evenly, so beside something drawn it's the law's and not a sample's.
                Some(law) => (0..DRAWS).map(|i| law.quantile((i as f64 + 0.5) / DRAWS as f64)).collect(),
                None => self.column(r).to_vec(),
            },
        }
    }

    /// What a number comes to, if it's each of these this much of the time. With one, that's what that
    /// one comes to. It's exact where each of them is a plain number or has a law, and otherwise it's
    /// read from draws.
    pub fn summary(&mut self, shares: &[Share]) -> Summary {
        let whole: f64 = shares.iter().map(|s| s.weight).sum();
        let live: Vec<&Share> = shares.iter().filter(|s| s.weight > 0.0).collect();
        let (mut least, mut most) = (f64::INFINITY, f64::NEG_INFINITY);
        for share in &live {
            let (lo, hi) = match &share.value {
                Term::Rand(r) => r.bounds(),
                Term::Num(n) => (*n, *n),
                _ => (f64::INFINITY, f64::NEG_INFINITY),
            };
            (least, most) = (least.min(lo), most.max(hi));
        }
        let (least, most) = (Some(least).filter(|v| v.is_finite()), Some(most).filter(|v| v.is_finite()));

        // Each as a law, if every one of them is one.
        let mut undefined = 0.0;
        let mut laws: Option<Vec<(f64, Law)>> = Some(Vec::new());
        for share in &live {
            let w = share.weight / whole;
            let law = match &share.value {
                Term::Undefined => {
                    undefined += w;
                    continue;
                }
                Term::Num(n) => Some(Law::Point(*n)),
                Term::Flag(b) => Some(Law::Point(f64::from(u8::from(*b)))),
                // Given something that shares a draw with it, it isn't the law it was.
                Term::Rand(r) if self.tied(r) => None,
                Term::Rand(r) => self.law(r),
            };
            laws = laws.zip(law).map(|(mut all, law)| {
                all.push((w, law));
                all
            });
        }
        if let Some(mut laws) = laws.filter(|l| !l.is_empty()) {
            let defined: f64 = laws.iter().map(|(w, _)| w).sum();
            laws.iter_mut().for_each(|(w, _)| *w /= defined);
            let law = if laws.len() == 1 { laws.remove(0).1 } else { Law::Mixed(laws) };
            let named = if matches!(law, Law::Point(_)) { None } else { law.text() };
            let hist = law.hist();
            let below0 = (hist.from < 0.0 && hist.to > 0.0).then(|| law.mass(0.0, false));
            return Summary { mean: law.mean(), sd: law.sd(), low: law.quantile(0.05), mid: law.quantile(0.5), high: law.quantile(0.95), least, most, undefined, hist, below0, law: named, exact: true, draws: 0, error: 0.0 };
        }

        let mut values = Vec::new();
        let mut undefined = 0.0;
        for share in &live {
            let spread = self.spread(&share.value);
            let each = share.weight / whole / spread.len() as f64;
            undefined += spread.iter().filter(|v| v.is_nan()).count() as f64 * each;
            values.extend(spread.into_iter().map(|v| (v, each)));
        }
        let values = Weighted::new(values);
        let sd = values.sd();
        // Given something, only the draws where it holds were read.
        let draws = if live.iter().any(|s| matches!(&s.value, Term::Rand(r) if self.tied(r))) { self.draws_kept() } else { DRAWS };
        let hist = values.hist();
        let below0 = (hist.from < 0.0 && hist.to > 0.0).then(|| values.0.iter().filter(|(v, _)| *v < 0.0).map(|(_, w)| w).sum::<f64>() / values.total());
        Summary { mean: values.mean(), sd, low: values.quantile(0.05), mid: values.quantile(0.5), high: values.quantile(0.95), least, most, undefined, hist, below0, law: None, exact: false, draws, error: sd / (draws.max(1) as f64).sqrt() }
    }

    /// How likely a truth is, if it's each of these this much of the time.
    pub fn chance(&mut self, shares: &[Share]) -> Chance {
        let whole: f64 = shares.iter().map(|s| s.weight).sum();
        let (mut yes, mut undefined, mut drawn, mut hits, mut read) = (0.0, 0.0, 0, 0, 0);
        for share in shares.iter().filter(|s| s.weight > 0.0) {
            let w = share.weight / whole;
            match &share.value {
                Term::Flag(true) => yes += w,
                Term::Rand(r) => {
                    let tied = self.tied(r);
                    // A law says how likely exactly, and what has one is never undefined. Given something
                    // that shares a draw with it, that's no longer how likely it is.
                    if !tied && let Some(sure) = self.sure(r.0, 0) {
                        yes += w * sure;
                        continue;
                    }
                    let (column, kept) = (self.column(r), self.kept());
                    let among = if tied { kept.iter().filter(|k| **k).count() } else { DRAWS };
                    let counts = |i: usize| !tied || kept[i];
                    let nan = column.iter().enumerate().filter(|(i, v)| counts(*i) && v.is_nan()).count();
                    let here = column.iter().enumerate().filter(|(i, v)| counts(*i) && **v == 1.0).count();
                    if among == 0 {
                        // What's known never came up, so there's nothing to say how likely this is given it.
                        undefined += w;
                    } else {
                        yes += w * here as f64 / among as f64;
                        undefined += w * nan as f64 / among as f64;
                    }
                    hits += here;
                    drawn += 1;
                    read += among;
                }
                Term::Undefined => undefined += w,
                _ => {}
            }
        }
        let chance = if undefined < 1.0 { yes / (1.0 - undefined) } else { 0.0 };
        // Each thing it could be that had to be drawn has its own draws, so "in every one" and "in none"
        // are about all of them.
        let exact = drawn == 0;
        let each = if drawn == 0 { DRAWS } else { (read / drawn).max(1) };
        Chance { chance, undefined, exact, draws: read, hits, error: if exact { 0.0 } else { (chance * (1.0 - chance) / each as f64).sqrt() } }
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
        let product = bin(Op::Mul, &a, &b);
        let mut reader = Reader::new();
        assert_eq!(reader.law(&rand(&product)), None);
        let s = reader.summary(&one(&product));
        assert!(!s.exact && s.draws == DRAWS && s.error > 0.0 && s.law.is_none());
        // Two unrelated bell curves multiplied are about nothing, give or take one.
        assert!(s.mean.abs() < 3.0 * s.error && (s.sd - 1.0).abs() < 0.1, "{s:?}");
        let c = reader.chance(&one(&bin(Op::Lt, &product, &n(0.0))));
        assert!(!c.exact && c.draws == DRAWS && (c.chance - 0.5).abs() < 3.0 * c.error, "{c:?}");
    }

    #[test]
    fn the_later_of_things_that_share_nothing_has_a_law() {
        let (a, b) = (normal("t/later/a", 0.0, 1.0), normal("t/later/b", 0.0, 1.0));
        let later = bin(Op::Max, &a, &b);
        let mut reader = Reader::new();
        assert_eq!(reader.law(&rand(&later)), Some(Law::Greatest(vec![Law::Normal { mean: 0.0, sd: 1.0 }, Law::Normal { mean: 0.0, sd: 1.0 }])));
        let s = reader.summary(&one(&later));
        assert!(s.exact && s.law.as_deref() == Some("max(normal(0, 1), normal(0, 1))"), "{s:?}");
        // The later of two is later than either, on average: 1/√π, give or take √(1 − 1/π).
        let pi = std::f64::consts::PI;
        assert!((s.mean - 1.0 / pi.sqrt()).abs() < 1e-9 && (s.sd - (1.0 - 1.0 / pi).sqrt()).abs() < 1e-8, "{s:?}");
        // Both are under a point exactly when the later is.
        let c = reader.chance(&one(&bin(Op::Lt, &later, &n(1.0))));
        assert!(c.exact && (c.chance - phi(1.0) * phi(1.0)).abs() < 1e-12, "{c:?}");
        // The earlier is the same thing turned over, and what shares a draw has no such law.
        let earlier = reader.summary(&one(&bin(Op::Min, &a, &b)));
        assert!(earlier.exact && (earlier.mean + 1.0 / pi.sqrt()).abs() < 1e-9, "{earlier:?}");
        assert_eq!(reader.law(&rand(&bin(Op::Max, &a, &bin(Op::Add, &a, &b)))), None);
    }

    #[test]
    fn a_number_held_at_nothing_or_above_has_weight_on_nothing() {
        // max(x, 0), as a task that can't end before it starts: half the time it's exactly 0.
        let held = bin(Op::Max, &normal("t/held/x", 0.0, 1.0), &n(0.0));
        let mut reader = Reader::new();
        let s = reader.summary(&one(&held));
        assert!(s.exact && s.law.as_deref() == Some("max(normal(0, 1), 0)") && s.mid == 0.0 && s.low == 0.0, "{s:?}");
        assert!((s.mean - 1.0 / (2.0 * std::f64::consts::PI).sqrt()).abs() < 1e-9, "{s:?}");
        let mut sure = |t: Term| {
            let c = reader.chance(&one(&t));
            assert!(c.exact, "{c:?}");
            c.chance
        };
        // At or below nothing half the time, below it never, and exactly it half the time.
        assert_eq!((sure(bin(Op::Le, &held, &n(0.0))), sure(bin(Op::Eq, &held, &n(0.0))), sure(bin(Op::Ne, &held, &n(0.0)))), (0.5, 0.5, 0.5));
        assert_eq!(bin(Op::Lt, &held, &n(0.0)), Term::Flag(false));
        assert!((sure(bin(Op::Lt, &n(1.0), &held)) - (1.0 - phi(1.0))).abs() < 1e-12);
    }

    #[test]
    fn products_and_powers_of_what_has_a_bell_curve_for_a_log_keep_a_law() {
        let (a, b) = (normal("t/log/a", 0.5, 0.3), normal("t/log/b", 0.1, 0.4));
        let (ea, eb) = (un(Un::Exp, &a), un(Un::Exp, &b));
        let mut reader = Reader::new();
        let mut law = |t: Term| reader.law(&rand(&t));
        let near = |law: Option<Law>, mu: f64, sigma: f64| assert!(matches!(law, Some(Law::Lognormal { mu: m, sigma: s }) if (m - mu).abs() < 1e-12 && (s - sigma).abs() < 1e-12), "{law:?} isn't lognormal({mu}, {sigma})");
        near(law(bin(Op::Mul, &ea, &eb)), 0.6, 0.5);
        near(law(bin(Op::Div, &ea, &eb)), 0.4, 0.5);
        near(law(bin(Op::Mul, &n(1000.0), &ea)), 0.5 + 1000f64.ln(), 0.3);
        near(law(bin(Op::Pow, &ea, &n(2.0))), 1.0, 0.6);
        near(law(un(Un::Sqrt, &ea)), 0.25, 0.15);
        // Growing by an uncertain rate each year, compounded: still one law after any number of years.
        let years = (0..10).fold(n(1000.0), |balance, year| bin(Op::Mul, &balance, &un(Un::Exp, &normal(&format!("t/log/year{year}"), 0.05, 0.1))));
        near(law(years), 1000f64.ln() + 0.5, 0.1 * 10f64.sqrt());
        // Which of two is the greater is which has the greater log, whatever they share.
        let ahead = reader.chance(&one(&bin(Op::Lt, &bin(Op::Mul, &ea, &eb), &bin(Op::Mul, &n(2.0), &eb))));
        assert!(ahead.exact && (ahead.chance - phi((2f64.ln() - 0.5) / 0.3)).abs() < 1e-12, "{ahead:?}");
        let mut law = |t: Term| reader.law(&rand(&t));
        // And the log of one is a bell curve again.
        assert_eq!(law(un(Un::Ln, &bin(Op::Mul, &ea, &ea))), Some(Law::Normal { mean: 1.0, sd: 0.6 }));
        // Moved along it has no name of its own, and is still read exactly.
        let moved = law(bin(Op::Add, &ea, &n(1.0))).unwrap();
        assert_eq!(moved.text().as_deref(), Some("1 + lognormal(0.5, 0.3)"));
        assert!((moved.mean() - (1.0 + (0.5f64 + 0.045).exp())).abs() < 1e-12 && (moved.below(1.0 + 0.5f64.exp()) - 0.5).abs() < 1e-12, "{moved:?}");
    }

    #[test]
    fn a_law_moved_or_stretched_by_a_plain_number_is_still_one() {
        let estimate = tri(&Term::Rand(draw(Std::Uniform, "t/moved/t")), &n(2.0), &n(3.0), &n(6.0));
        let wait = Term::Rand(draw(Std::Exponential, "t/moved/e"));
        let mut reader = Reader::new();
        let mut law = |t: Term| reader.law(&rand(&t));
        assert_eq!(law(bin(Op::Add, &bin(Op::Mul, &n(2.0), &estimate), &n(1.0))), Some(Law::Triangular { low: 5.0, likely: 7.0, high: 13.0 }));
        assert_eq!(law(bin(Op::Sub, &n(10.0), &estimate)), Some(Law::Triangular { low: 4.0, likely: 7.0, high: 8.0 }));
        assert_eq!(law(bin(Op::Div, &estimate, &n(2.0))), Some(Law::Triangular { low: 1.0, likely: 1.5, high: 3.0 }));
        assert_eq!(law(bin(Op::Add, &n(3.0), &bin(Op::Mul, &n(2.0), &wait))), Some(Law::Exponential { from: 3.0, mean: 2.0 }));
        // A wait turned over has no name of its own, and is still read exactly: all of it is below nothing.
        let turned = law(bin(Op::Sub, &n(10.0), &bin(Op::Mul, &n(2.0), &wait))).unwrap();
        assert_eq!(turned.text().as_deref(), Some("10 - exponential(2)"));
        assert!(turned.mean() == 8.0 && turned.sd() == 2.0 && turned.below(10.0) == 1.0 && (turned.below(8.0) - (-1f64).exp()).abs() < 1e-12, "{turned:?}");
        assert!((turned.below(turned.quantile(0.3)) - 0.3).abs() < 1e-12);
        // The later of two, moved, and turned over into the earlier of two.
        let later = bin(Op::Max, &estimate, &n(4.0));
        assert_eq!(law(bin(Op::Add, &later, &n(1.0))), Some(Law::Greatest(vec![Law::Triangular { low: 3.0, likely: 4.0, high: 7.0 }, Law::Point(5.0)])));
        assert_eq!(law(un(Un::Neg, &later)), Some(Law::Least(vec![Law::Triangular { low: -6.0, likely: -3.0, high: -2.0 }, Law::Point(-4.0)])));
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        /// A plain number from `from` to `to`, to one decimal place.
        fn between(&mut self, from: f64, to: f64) -> f64 {
            from + (self.next(((to - from) * 10.0) as usize + 1) as f64) / 10.0
        }
    }

    /// Something uncertain with a law, on a draw of its own. `above` asks for one that's never below nothing.
    fn any_law(rng: &mut Rng, case: usize, part: &mut usize, above: bool) -> Term {
        *part += 1;
        let name = format!("t/any/{case}/{part}");
        match rng.next(if above { 3 } else { 5 }) {
            0 => un(Un::Exp, &normal(&name, rng.between(-0.5, 1.0), rng.between(0.1, 0.4))),
            1 => {
                let low = rng.between(0.5, 3.0);
                let likely = low + rng.between(0.5, 2.0);
                tri(&Term::Rand(draw(Std::Uniform, &name)), &n(low), &n(likely), &n(likely + rng.between(0.5, 4.0)))
            }
            2 => bin(Op::Add, &n(rng.between(0.0, 2.0)), &bin(Op::Mul, &n(rng.between(0.5, 3.0)), &Term::Rand(draw(Std::Exponential, &name)))),
            3 => normal(&name, rng.between(-3.0, 3.0), rng.between(0.5, 2.0)),
            _ => bin(Op::Add, &n(rng.between(-3.0, 3.0)), &bin(Op::Mul, &n(rng.between(0.5, 4.0)), &Term::Rand(draw(Std::Uniform, &name)))),
        }
    }

    /// Something made of those in a way that keeps a law.
    fn made(rng: &mut Rng, case: usize, part: &mut usize, depth: usize) -> Term {
        if depth == 0 {
            return any_law(rng, case, part, false);
        }
        match rng.next(6) {
            // Moved and stretched, which turns the later of two into the earlier.
            0 => {
                let inner = bin(Op::Max, &made(rng, case, part, depth - 1), &any_law(rng, case, part, false));
                bin(Op::Add, &bin(Op::Mul, &n(if rng.next(2) == 0 { -1.5 } else { 2.0 }), &inner), &n(rng.between(-2.0, 2.0)))
            }
            1 => bin(Op::Max, &made(rng, case, part, depth - 1), &made(rng, case, part, depth - 1)),
            2 => bin(Op::Min, &made(rng, case, part, depth - 1), &made(rng, case, part, depth - 1)),
            // Held at or above a plain number, so some of its weight is on that number.
            3 => bin(Op::Max, &made(rng, case, part, depth - 1), &n(rng.between(0.0, 3.0))),
            // A sum of bell curves, one of them counted twice.
            4 => {
                *part += 3;
                let (a, b) = (normal(&format!("t/any/{case}/{part}a"), rng.between(-2.0, 2.0), rng.between(0.5, 2.0)), normal(&format!("t/any/{case}/{part}b"), rng.between(-2.0, 2.0), rng.between(0.5, 2.0)));
                bin(Op::Sub, &bin(Op::Add, &bin(Op::Mul, &n(2.0), &a), &b), &bin(Op::Div, &a, &n(4.0)))
            }
            // A product of things whose logs are bell curves.
            _ => {
                *part += 3;
                let (a, b) = (un(Un::Exp, &normal(&format!("t/any/{case}/{part}a"), rng.between(-0.5, 0.5), rng.between(0.1, 0.3))), un(Un::Exp, &normal(&format!("t/any/{case}/{part}b"), rng.between(-0.5, 0.5), rng.between(0.1, 0.3))));
                bin(Op::Div, &bin(Op::Mul, &n(rng.between(0.5, 3.0)), &bin(Op::Mul, &a, &b)), &un(Un::Sqrt, &b))
            }
        }
    }

    #[test]
    fn every_law_found_agrees_with_drawing_what_it_was_found_for() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let (mut held, mut both_ways) = (0, 0);
        for case in 0..300 {
            let mut part = 0;
            let depth = rng.next(3);
            let term = made(&mut rng, case, &mut part, depth);
            let Term::Rand(r) = term else { continue };
            let mut reader = Reader::new();
            let law = reader.law(&r).unwrap_or_else(|| panic!("case {case} should have a law"));
            let column = Weighted::new(reader.column(&r).iter().map(|v| (*v, 1.0)).collect());
            assert_eq!(column.0.len(), DRAWS, "case {case}: {law:?} is never undefined");
            let (mean, sd) = (law.mean(), law.sd());
            assert!((column.mean() - mean).abs() < 0.08 * sd + 1e-9, "case {case} {law:?}: mean {} isn't {mean}", column.mean());
            // Where nearly all the weight is on one number, the little that isn't may not come up in the
            // draws at all, which is the draws' doing: so within a little, as well as within a share.
            assert!((column.sd() - sd).abs() < 0.12 * sd + 0.005, "case {case} {law:?}: spread {} isn't {sd}", column.sd());
            let mut points = Vec::new();
            law.points(&mut points);
            held += usize::from(!points.is_empty());
            both_ways += usize::from(matches!(law, Law::Least(_)));
            for p in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let at = law.quantile(p);
                // As much is below a point as the point was asked to have below it, unless weight sits on it.
                assert!(law.mass(at, true) >= p - 1e-9 && law.mass(at, false) <= p + 1e-9, "case {case} {law:?} at {p}: {at} has {} to {} below", law.mass(at, false), law.mass(at, true));
                let drawn = column.0.iter().filter(|(v, _)| *v <= at).count() as f64 / DRAWS as f64;
                assert!((drawn - law.mass(at, true)).abs() < 0.035, "case {case} {law:?}: {drawn} of the draws are at or below {at}, not {}", law.mass(at, true));
                // And a question about it is answered the same way the law would.
                let c = reader.chance(&[Share { weight: 1.0, value: bin(Op::Le, &Term::Rand(r), &n(at)) }]);
                assert!(c.exact && (c.chance - law.mass(at, true)).abs() < 1e-12, "case {case} {law:?} at {at}: {c:?}");
            }
        }
        assert!(held >= 25 && both_ways >= 25, "the generator should make every kind: {held} held, {both_ways} earliest");
    }

    #[test]
    fn questions_about_things_that_share_nothing_multiply() {
        let (a, b) = (normal("t/apart/a", 0.0, 1.0), normal("t/apart/b", 0.0, 1.0));
        let (first, second) = (bin(Op::Lt, &a, &n(1.0)), bin(Op::Lt, &n(0.0), &b));
        let mut reader = Reader::new();
        let both = reader.chance(&one(&bin(Op::And, &first, &second)));
        assert!(both.exact && (both.chance - phi(1.0) * 0.5).abs() < 1e-12, "{both:?}");
        let either = reader.chance(&one(&bin(Op::Or, &first, &second)));
        assert!(either.exact && (either.chance - (1.0 - (1.0 - phi(1.0)) * 0.5)).abs() < 1e-12, "{either:?}");
        // About things that share a draw, they don't, so that's read from draws.
        let tied = reader.chance(&one(&bin(Op::And, &first, &bin(Op::Lt, &n(0.0), &bin(Op::Add, &a, &b)))));
        assert!(!tied.exact && tied.draws == DRAWS, "{tied:?}");
        // One number against itself moved along is settled whatever it is.
        let moved = reader.chance(&one(&bin(Op::Lt, &bin(Op::Mul, &a, &n(2.0)), &bin(Op::Add, &bin(Op::Mul, &a, &n(2.0)), &n(1.0)))));
        assert!(moved.exact && moved.chance == 1.0, "{moved:?}");
    }

    #[test]
    fn simplifying_changes_nothing_in_any_draw() {
        let x = normal("t/same2/x", 0.0, 1.0);
        let u = Term::Rand(draw(Std::Uniform, "t/same2/u"));
        let (under, over) = (bin(Op::Lt, &x, &n(1.0)), bin(Op::Lt, &n(1.0), &x));
        assert_eq!(un(Un::Not, &under), bin(Op::Le, &n(1.0), &x), "not below is at or above");
        assert_eq!(bin(Op::And, &under, &un(Un::Not, &under)), Term::Flag(false));
        assert_eq!(bin(Op::Or, &under, &un(Un::Not, &under)), Term::Flag(true));
        assert_eq!(pick(&un(Un::Not, &over), &x, &n(1.0)), pick(&over, &n(1.0), &x), "choosing on the opposite is choosing the other way round");
        assert_eq!(pick(&bin(Op::Le, &x, &n(1.0)), &x, &n(1.0)), pick(&over, &n(1.0), &x), "and at or below is the opposite of above");
        assert_eq!(bin(Op::Sub, &n(0.0), &x), un(Un::Neg, &x));
        assert_eq!(un(Un::Floor, &un(Un::Ceil, &x)), un(Un::Ceil, &x));
        let above = bin(Op::Add, &n(1.0), &u);
        assert_eq!(bin(Op::Div, &above, &above), n(1.0), "a number over itself is one, where it's never nothing");
        assert!(matches!(bin(Op::Div, &x, &x), Term::Rand(_)), "and isn't where it could be");
        // Undefined in some draws, none of that holds, so it's left alone.
        let root = un(Un::Sqrt, &x);
        assert!(matches!(bin(Op::Sub, &root, &root), Term::Rand(_)));
        let (below, opposite) = (rand(&bin(Op::Lt, &root, &n(1.0))), rand(&un(Un::Not, &bin(Op::Lt, &root, &n(1.0)))));
        let held = arena().nodes[opposite.0 as usize].clone();
        assert_eq!(held, Node::Un(Un::Not, below.0), "not below isn't at or above where it could be neither");
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
    fn how_much_is_below_zero_is_said_where_it_crosses_zero() {
        let mut reader = Reader::new();
        let s = reader.summary(&one(&normal("t/zero/x", 0.05, 0.1)));
        assert!((s.below0.unwrap() - phi(-0.5)).abs() < 1e-12, "{s:?}");
        assert_eq!(reader.summary(&one(&normal("t/zero/y", 5.0, 1.0))).below0, None, "it doesn't come near");
        let drawn = reader.summary(&one(&bin(Op::Mul, &normal("t/zero/a", 0.0, 1.0), &normal("t/zero/b", 0.0, 1.0))));
        assert!(!drawn.exact && (drawn.below0.unwrap() - 0.5).abs() < 0.03, "{drawn:?}");
    }

    #[test]
    fn drawing_is_the_same_every_time() {
        let x = bin(Op::Mul, &normal("t/again/a", 0.0, 1.0), &normal("t/again/b", 0.0, 1.0));
        let (first, second) = (Reader::new().summary(&one(&x)), Reader::new().summary(&one(&x)));
        assert!(!first.exact);
        assert_eq!(first, second);
    }

    #[test]
    fn a_mixture_weighs_each_by_how_often() {
        let mut reader = Reader::new();
        let s = reader.summary(&[Share { weight: 3.0, value: n(10.0) }, Share { weight: 1.0, value: n(20.0) }]);
        assert!(s.exact && s.mean == 12.5 && s.mid == 10.0, "{s:?}");
        // A mixture of laws is read exactly, though it has no name: either of two bell curves ten apart.
        let s = reader.summary(&[Share { weight: 1.0, value: normal("t/mix/a", 0.0, 1.0) }, Share { weight: 1.0, value: normal("t/mix/b", 10.0, 1.0) }]);
        assert!(s.law.is_none() && s.exact && s.mean == 5.0 && (s.sd - 26f64.sqrt()).abs() < 1e-12 && (s.mid - 5.0).abs() < 1e-6, "{s:?}");
        // A plain number some of the time puts weight on that number.
        let s = reader.summary(&[Share { weight: 3.0, value: n(30.0) }, Share { weight: 1.0, value: normal("t/mix/c", 50.0, 5.0) }]);
        assert!(s.exact && s.mean == 35.0 && s.mid == 30.0 && s.low == 30.0, "{s:?}");
        // With one that has no law among them, it's all read from draws.
        let lawless = bin(Op::Mul, &normal("t/mix/d", 0.0, 1.0), &normal("t/mix/e", 0.0, 1.0));
        let s = reader.summary(&[Share { weight: 1.0, value: lawless }, Share { weight: 1.0, value: n(10.0) }]);
        assert!(!s.exact && s.draws == DRAWS && (s.mean - 5.0).abs() < 0.05, "{s:?}");
        // True in every draw one way and in none the other is half the time, not "in every draw".
        let (x, y) = (normal("t/mix/x", 0.0, 1.0), normal("t/mix/y", 0.0, 1.0));
        let (always, never) = (bin(Op::Le, &n(0.0), &bin(Op::Mul, &bin(Op::Max, &x, &y), &bin(Op::Max, &x, &y))), bin(Op::Lt, &bin(Op::Mul, &bin(Op::Max, &x, &y), &bin(Op::Max, &x, &y)), &n(0.0)));
        let c = reader.chance(&[Share { weight: 1.0, value: always }, Share { weight: 1.0, value: never }]);
        assert!(!c.exact && c.chance == 0.5 && c.hits == DRAWS && c.draws == 2 * DRAWS, "{c:?}");
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
