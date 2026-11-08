//! The formula language: its values, its types and its grammar. See `docs/expressions.md`. Nothing
//! here knows about graphs. `formulas.rs` is what gives a formula's names something to refer to.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use recollect_graph::Inputs;
use serde::{Deserialize, Serialize};

/// A value in one world. Undefined is `null`, the same as `None` in a `Cond<Option<T>>`, so what a
/// formula gives and what a compiled rule gives read the same.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Val {
    Undefined,
    Bool(bool),
    Num(f64),
    Text(String),
    List(Vec<Val>),
    Record(BTreeMap<String, Val>),
}

impl Val {
    /// A number, or undefined if it isn't one, so a `Cond` always has values that compare equal.
    pub fn num(n: f64) -> Val {
        if n.is_finite() { Val::Num(n) } else { Val::Undefined }
    }

    pub fn field(&self, name: &str) -> Val {
        match self {
            Val::Record(fields) => fields.get(name).cloned().unwrap_or(Val::Undefined),
            _ => Val::Undefined,
        }
    }
}

/// What a value is in every world. It doesn't say whether a value is defined, which is per world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Type {
    /// Not known here, such as a component nothing has said the type of. Checked as it runs.
    Any,
    /// Undefined in every world, which goes with anything.
    Undefined,
    Bool,
    Number,
    Text,
    /// One of a `Domain`'s options that aren't numbers.
    Enum(Vec<String>),
    List,
    Record(BTreeMap<String, Type>),
}

impl Type {
    pub fn of(v: &Val) -> Type {
        match v {
            Val::Undefined => Type::Undefined,
            Val::Bool(_) => Type::Bool,
            Val::Num(_) => Type::Number,
            Val::Text(_) => Type::Text,
            Val::List(_) => Type::List,
            Val::Record(fields) => Type::Record(fields.iter().map(|(k, v)| (k.clone(), Type::of(v))).collect()),
        }
    }

    /// What's the same about these two, if anything is.
    pub fn join(&self, other: &Type) -> Option<Type> {
        use Type::*;
        Some(match (self, other) {
            (Undefined, t) | (t, Undefined) => t.clone(),
            (Any, _) | (_, Any) => Any,
            (Enum(a), Enum(b)) => {
                let mut all = a.clone();
                all.extend(b.iter().filter(|o| !a.contains(o)).cloned());
                Enum(all)
            }
            (Enum(_), Text) | (Text, Enum(_)) => Text,
            (a, b) if a == b => a.clone(),
            (Record(_), Record(_)) => Any,
            _ => return None,
        })
    }

    pub fn word(&self) -> &'static str {
        match self {
            Type::Any => "anything",
            Type::Undefined => "undefined",
            Type::Bool => "true or false",
            Type::Number => "a number",
            Type::Text | Type::Enum(_) => "text",
            Type::List => "a list",
            Type::Record(_) => "a record",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Max,
    Min,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Eq => "=",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Max => "max",
            BinOp::Min => "min",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Lit(Val),
    /// `rate`, `rate.finish`, `self.need`, `whole.plan_end`.
    Path(Vec<String>),
    /// `Year[3].balance`: a row of a table, by its key, and a cell on it. See `table`.
    Row { table: String, key: Box<Expr>, path: Vec<String> },
    Call(String, Vec<Expr>),
    /// `sum(x in group: body)`. A group is `in.kind`, `out.kind`, `with.component`, or a name.
    /// `start` is what `max` and `min` begin from, and what `pick` gives if nothing's chosen.
    Fold { func: String, var: String, group: Vec<String>, body: Box<Expr>, start: Option<Box<Expr>> },
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

/// What's evaluated as itself, rather than being expanded into something else.
pub const CORE: &[&str] = &["if", "defined", "has", "chose", "round", "floor", "ceil", "max", "min", "distinct"];

/// Functions that expand into one copy of their body for each node in a group.
pub const FOLDS: &[&str] = &["sum", "product", "all", "any", "count", "pick", "max", "min", "distinct"];

/// Folds with no answer of their own for an empty group, so they have to be told where to begin.
const NEEDS_START: &[&str] = &["max", "min"];

/// The rest of the library, written in the language itself, with no special standing.
const LIBRARY: &[(&str, &[&str], &str)] = &[
    ("abs", &["x"], "if(x < 0, -x, x)"),
    ("or", &["x", "fallback"], "if(defined(x), x, fallback)"),
    // A plan's effort in each world. Summing over tasks, not up through parents, counts shared work
    // once. A gap in play means nobody knows, and a task with no estimate is the same.
    ("effort", &[], "if(any(u in with.unknown: u.need), undefined, sum(t in with.task: if(t.need, t.estimate, 0)))"),
    // Whether this node's own time is its own, because nothing beneath it stands in for it.
    ("is_leaf", &[], "count(p in in.part_of: !p.dropped) = 0 && !has(self.decision)"),
    // Whether what's beneath is finished. What has nothing to finish has no say, and if nothing does,
    // neither does this.
    ("done_beneath", &["chosen"], "if(any(p in in.part_of: defined(p.done)) || defined(chosen), all(p in in.part_of: or(p.done, true)) && or(chosen, true), undefined)"),
    // How long this node takes. With no estimate that's nothing, and an estimate a formula can't
    // work out makes the dates unknowable.
    ("days", &[], "if(has(self.estimate), self.estimate, 0)"),
];

pub struct LibraryMacro {
    pub params: Vec<String>,
    pub body: Expr,
}

static PARSED_LIBRARY: LazyLock<BTreeMap<&'static str, LibraryMacro>> = LazyLock::new(|| {
    LIBRARY
        .iter()
        .map(|(name, params, body)| {
            let m = LibraryMacro { params: params.iter().map(|p| p.to_string()).collect(), body: parse(body).expect("the library parses") };
            (*name, m)
        })
        .collect()
});

pub fn library(name: &str) -> Option<&'static LibraryMacro> {
    PARSED_LIBRARY.get(name)
}

/// What a formula may follow: labelled links always, and whatever its groups name.
pub fn inputs_of(e: &Expr) -> Inputs {
    fn add(list: &mut Vec<String>, kind: &str) {
        if !list.iter().any(|k| k == kind) {
            list.push(kind.to_string());
        }
    }
    fn walk(e: &Expr, i: &mut Inputs) {
        match e {
            Expr::Lit(_) | Expr::Path(_) | Expr::Row { .. } => {}
            Expr::Not(a) | Expr::Neg(a) => walk(a, i),
            Expr::Bin(_, a, b) => {
                walk(a, i);
                walk(b, i);
            }
            Expr::Call(name, args) => {
                if let Some(m) = library(name) {
                    walk(&m.body, i);
                }
                args.iter().for_each(|a| group_arg(a, i));
                args.iter().for_each(|a| walk(a, i));
            }
            Expr::Fold { group, body, start, .. } => {
                group_path(group, i);
                walk(body, i);
                if let Some(start) = start {
                    walk(start, i);
                }
            }
        }
    }
    // A macro's parameter can be a group, so anything shaped like one counts.
    fn group_arg(e: &Expr, i: &mut Inputs) {
        if let Expr::Path(p) = e {
            group_path(p, i);
        }
    }
    fn group_path(p: &[String], i: &mut Inputs) {
        match p {
            [root, kind, ..] if root == "in" => add(&mut i.incoming, kind),
            [root, kind, ..] if root == "out" => add(&mut i.outgoing, kind),
            [root, kind, ..] if root == "with" => add(&mut i.scans, kind),
            _ => {}
        }
    }
    let mut i = Inputs { outgoing: vec!["label".to_string()], ..Inputs::default() };
    walk(e, &mut i);
    i
}

// ---------- reading it ----------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Name(String),
    Sym(&'static str),
}

const SYMBOLS: &[&str] = &["&&", "||", "!=", "<=", ">=", "(", ")", "[", "]", ",", ".", ":", "+", "-", "*", "/", "^", "=", "<", ">", "!"];

fn tokens(text: &str) -> Result<Vec<(Tok, usize)>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at = i + 1;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))) {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push((Tok::Num(s.parse().map_err(|_| format!("{s} at column {at} isn't a number"))?), at));
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err(format!("the text starting at column {at} never ends")),
                    Some('"') => break,
                    Some('\\') => {
                        s.extend(chars.get(i + 1));
                        i += 2;
                    }
                    Some(c) => {
                        s.push(*c);
                        i += 1;
                    }
                }
            }
            i += 1;
            out.push((Tok::Str(s), at));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push((Tok::Name(chars[start..i].iter().collect()), at));
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let Some(sym) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
                return Err(format!("didn't expect “{c}” at column {at}"));
            };
            i += sym.chars().count();
            out.push((Tok::Sym(sym), at));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    at: usize,
    end: usize,
    depth: usize,
}

/// More nested than anyone writes, and few enough that reading it can't run out of stack.
const MOST_DEPTH: usize = 100;

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|t| &t.0)
    }

    fn column(&self) -> usize {
        self.toks.get(self.at).map_or(self.end, |t| t.1)
    }

    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, sym: &str) -> Result<(), String> {
        if self.eat(sym) { Ok(()) } else { Err(format!("expected “{sym}” at column {}", self.column())) }
    }

    fn name(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(Tok::Name(n)) => {
                let n = n.clone();
                self.at += 1;
                Ok(n)
            }
            _ => Err(format!("expected a name at column {}", self.column())),
        }
    }

    fn binary(&mut self, ops: &[(&str, BinOp)], next: fn(&mut Parser) -> Result<Expr, String>) -> Result<Expr, String> {
        let mut left = next(self)?;
        'more: loop {
            for (sym, op) in ops {
                if self.eat(sym) {
                    left = Expr::Bin(*op, Box::new(left), Box::new(next(self)?));
                    continue 'more;
                }
            }
            return Ok(left);
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > MOST_DEPTH {
            return Err(format!("it's too deeply nested at column {}", self.column()));
        }
        let e = self.binary(&[("||", BinOp::Or)], Parser::and);
        self.depth -= 1;
        e
    }

    fn and(&mut self) -> Result<Expr, String> {
        self.binary(&[("&&", BinOp::And)], Parser::not)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat("!") {
            self.deeper()?;
            let e = Expr::Not(Box::new(self.not()?));
            self.depth -= 1;
            return Ok(e);
        }
        self.compare()
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MOST_DEPTH { Err(format!("it's too deeply nested at column {}", self.column())) } else { Ok(()) }
    }

    fn compare(&mut self) -> Result<Expr, String> {
        let left = self.sum()?;
        for (sym, op) in [("=", BinOp::Eq), ("!=", BinOp::Ne), ("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)] {
            if self.eat(sym) {
                return Ok(Expr::Bin(op, Box::new(left), Box::new(self.sum()?)));
            }
        }
        Ok(left)
    }

    fn sum(&mut self) -> Result<Expr, String> {
        self.binary(&[("+", BinOp::Add), ("-", BinOp::Sub)], Parser::term)
    }

    fn term(&mut self) -> Result<Expr, String> {
        self.binary(&[("*", BinOp::Mul), ("/", BinOp::Div)], Parser::unary)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            self.deeper()?;
            let e = Expr::Neg(Box::new(self.unary()?));
            self.depth -= 1;
            return Ok(e);
        }
        self.power()
    }

    /// `a ^ b`, from the right as it's usually read, so `2 ^ 3 ^ 2` is `2 ^ 9`, and `-2 ^ 2` is `-(2 ^ 2)`.
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if !self.eat("^") {
            return Ok(base);
        }
        self.deeper()?;
        let e = Expr::Bin(BinOp::Pow, Box::new(base), Box::new(self.unary()?));
        self.depth -= 1;
        Ok(e)
    }

    fn path_from(&mut self, first: String) -> Result<Vec<String>, String> {
        let mut path = vec![first];
        while self.eat(".") {
            path.push(self.name()?);
        }
        Ok(path)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let column = self.column();
        let Some((tok, _)) = self.toks.get(self.at).cloned() else {
            return Err(if self.toks.is_empty() { "it's empty".to_string() } else { format!("it stops short at column {column}") });
        };
        self.at += 1;
        match tok {
            Tok::Num(n) => Ok(Expr::Lit(Val::Num(n))),
            Tok::Str(s) => Ok(Expr::Lit(Val::Text(s))),
            Tok::Sym("(") => {
                let e = self.or()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Sym(s) => Err(format!("didn't expect “{s}” at column {column}")),
            Tok::Name(n) => match n.as_str() {
                "true" => Ok(Expr::Lit(Val::Bool(true))),
                "false" => Ok(Expr::Lit(Val::Bool(false))),
                "undefined" => Ok(Expr::Lit(Val::Undefined)),
                _ if self.eat("(") => self.call(n),
                _ if self.eat("[") => {
                    self.deeper()?;
                    let key = self.or()?;
                    self.depth -= 1;
                    self.expect("]")?;
                    let mut path = Vec::new();
                    while self.eat(".") {
                        path.push(self.name()?);
                    }
                    Ok(Expr::Row { table: n, key: Box::new(key), path })
                }
                _ => Ok(Expr::Path(self.path_from(n)?)),
            },
        }
    }

    fn call(&mut self, func: String) -> Result<Expr, String> {
        // `x in group: body`
        if let (Some(Tok::Name(var)), Some(Tok::Name(word))) = (self.toks.get(self.at).map(|t| &t.0), self.toks.get(self.at + 1).map(|t| &t.0))
            && word == "in"
        {
            let var = var.clone();
            self.at += 2;
            if !FOLDS.contains(&func.as_str()) {
                return Err(format!("{func} doesn't go over a group"));
            }
            let first = self.name()?;
            let group = self.path_from(first)?;
            self.expect(":")?;
            let body = self.or()?;
            let start = if self.eat(",") { Some(Box::new(self.or()?)) } else { None };
            self.expect(")")?;
            check_start(&func, start.is_some())?;
            return Ok(Expr::Fold { func, var, group, body: Box::new(body), start });
        }
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                args.push(self.or()?);
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        // `max(a, b)` is two numbers, and only a group spelled out as one can be gone over.
        let of_group = matches!(args.first(), Some(Expr::Path(p)) if matches!(p[0].as_str(), "in" | "out" | "with" | "rows") || p.get(1).is_some_and(|r| r == "rows"));
        // `distinct(a, b, c)` is the values given, and `distinct(group)` goes over the group.
        let many = func == "distinct" && args.len() != 1;
        if FOLDS.contains(&func.as_str()) && (!NEEDS_START.contains(&func.as_str()) || of_group) && !many {
            return short_fold(func, args);
        }
        Ok(Expr::Call(func, args))
    }
}

fn check_start(func: &str, given: bool) -> Result<(), String> {
    if given && !(NEEDS_START.contains(&func) || func == "pick") {
        return Err(format!("{func} takes one group, such as {func}(line) or {func}(x in line: x)"));
    }
    if !given && NEEDS_START.contains(&func) {
        return Err(format!("{func} over a group needs somewhere to begin, such as {func}(x in line: x, 0)"));
    }
    Ok(())
}

/// `sum(line.cost)` is short for `sum(x in line: x.cost)`. `count(line)` is how many there are.
fn short_fold(func: String, mut args: Vec<Expr>) -> Result<Expr, String> {
    let start = if args.len() == 2 { args.pop().map(Box::new) } else { None };
    check_start(&func, start.is_some())?;
    let (1, Some(Expr::Path(path))) = (args.len(), args.pop()) else {
        return Err(format!("{func} takes one group, such as {func}(line) or {func}(x in line: x)"));
    };
    let whole = if matches!(path[0].as_str(), "in" | "out" | "with") || path.get(1).is_some_and(|p| p == "rows") { 2 } else { 1 };
    if path.len() < whole {
        return Err(format!("{} needs to say which kind", path[0]));
    }
    let var = "it".to_string();
    let mut read = vec![var.clone()];
    read.extend_from_slice(&path[whole..]);
    let body = if func == "count" && path.len() == whole { Expr::Lit(Val::Bool(true)) } else { Expr::Path(read) };
    Ok(Expr::Fold { func, var, group: path[..whole].to_vec(), body: Box::new(body), start })
}

pub fn parse(text: &str) -> Result<Expr, String> {
    let toks = tokens(text)?;
    let end = text.chars().count() + 1;
    let mut p = Parser { toks, at: 0, end, depth: 0 };
    let e = p.or()?;
    if p.at < p.toks.len() {
        return Err(format!("didn't expect anything more at column {}", p.column()));
    }
    Ok(e)
}

/// Whether this can be a label: it has to read as a single name in a formula.
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    starts && chars.all(|c| c.is_alphanumeric() || c == '_') && !matches!(s, "true" | "false" | "undefined" | "self" | "whole" | "in" | "out" | "with" | "prev" | "next" | "row" | "rows")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> Expr {
        Expr::Path(s.split('.').map(String::from).collect())
    }

    #[test]
    fn precedence_is_what_you_would_expect() {
        let e = parse("a + b * c").unwrap();
        assert_eq!(e, Expr::Bin(BinOp::Add, Box::new(path("a")), Box::new(Expr::Bin(BinOp::Mul, Box::new(path("b")), Box::new(path("c"))))));
        let e = parse("!a = b && c").unwrap();
        let eq = Expr::Bin(BinOp::Eq, Box::new(path("a")), Box::new(path("b")));
        assert_eq!(e, Expr::Bin(BinOp::And, Box::new(Expr::Not(Box::new(eq))), Box::new(path("c"))));
        assert_eq!(parse("(a)").unwrap(), path("a"));
        assert_eq!(parse("1 - 2 - 3").unwrap(), Expr::Bin(BinOp::Sub, Box::new(Expr::Bin(BinOp::Sub, Box::new(Expr::Lit(Val::Num(1.0))), Box::new(Expr::Lit(Val::Num(2.0))))), Box::new(Expr::Lit(Val::Num(3.0)))), "left to right");
        let pow = |a: Expr, b: Expr| Expr::Bin(BinOp::Pow, Box::new(a), Box::new(b));
        let n = |x: f64| Expr::Lit(Val::Num(x));
        assert_eq!(parse("2 ^ 3 ^ 2").unwrap(), pow(n(2.0), pow(n(3.0), n(2.0))), "a power from the right");
        assert_eq!(parse("-a ^ 2").unwrap(), Expr::Neg(Box::new(pow(path("a"), n(2.0)))), "before negating");
        assert_eq!(parse("a * b ^ -1").unwrap(), Expr::Bin(BinOp::Mul, Box::new(path("a")), Box::new(pow(path("b"), Expr::Neg(Box::new(n(1.0)))))), "and before multiplying");
    }

    #[test]
    fn a_group_can_be_written_short_or_long() {
        assert_eq!(parse("sum(line.cost)").unwrap(), parse("sum(it in line: it.cost)").unwrap());
        assert_eq!(parse("sum(in.part_of.days)").unwrap(), parse("sum(it in in.part_of: it.days)").unwrap());
        assert!(parse("sum(1, 2)").is_err());
        assert!(parse("abs(x in y: x)").is_err());
    }

    #[test]
    fn max_and_min_are_two_numbers_or_a_group_from_somewhere() {
        assert!(matches!(parse("max(a, b)").unwrap(), Expr::Call(..)), "a name could be a number");
        assert_eq!(parse("max(out.dep.finish, 0)").unwrap(), parse("max(it in out.dep: it.finish, 0)").unwrap());
        assert!(matches!(parse("min(x in line: x, 1)").unwrap(), Expr::Fold { start: Some(_), .. }));
        assert!(parse("max(x in line: x)").unwrap_err().contains("somewhere to begin"));
        assert!(parse("max(out.dep.finish)").unwrap_err().contains("somewhere to begin"));
        assert!(matches!(parse("pick(x in line: x, 0)").unwrap(), Expr::Fold { start: Some(_), .. }), "what it gives if nothing's chosen");
        assert!(matches!(parse("pick(x in line: x)").unwrap(), Expr::Fold { start: None, .. }));
        assert!(parse("sum(x in line: x, 0)").is_err(), "a sum already has somewhere to begin");
        assert!(parse("sum(line, 0)").is_err());
    }

    #[test]
    fn it_says_where_it_went_wrong() {
        for (text, says) in [("a +", "stops short at column 4"), ("(a", "expected “)” at column 3"), ("a b", "column 3"), ("a # b", "“#” at column 3"), ("\"open", "never ends"), ("", "empty"), ("a = b = c", "column 7")] {
            let e = parse(text).unwrap_err();
            assert!(e.contains(says), "{text}: {e}");
        }
    }

    #[test]
    fn something_absurdly_nested_is_refused_and_doesnt_run_out_of_stack() {
        let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse(&deep).unwrap_err().contains("too deeply nested"));
        assert!(parse(&"!".repeat(100_000)).unwrap_err().contains("too deeply nested"));
        assert!(parse(&"-".repeat(100_000)).unwrap_err().contains("too deeply nested"));
        let nested_ok = format!("{}1{}", "(".repeat(50), ")".repeat(50));
        assert!(parse(&nested_ok).is_ok());
    }

    #[test]
    fn what_a_formula_follows_comes_from_what_it_says() {
        let i = inputs_of(&parse("sum(in.part_of.days) + count(out.depends_on) + all(with.task)").unwrap());
        assert_eq!((i.incoming, i.outgoing, i.scans), (vec!["part_of".to_string()], vec!["label".to_string(), "depends_on".to_string()], vec!["task".to_string()]));
        let i = inputs_of(&parse("effort()").unwrap());
        assert_eq!(i.scans, vec!["unknown".to_string(), "task".to_string()], "and whatever the library says for it");
    }

    #[test]
    fn labels_have_to_read_as_names() {
        assert!(is_name("rate") && is_name("rate_2"));
        assert!(!is_name("2rate") && !is_name("a b") && !is_name("") && !is_name("self") && !is_name("whole") && !is_name("in") && !is_name("true"));
    }
}
