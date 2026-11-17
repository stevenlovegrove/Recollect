//! A node named `Year[3]` is row 3 of the table `Year`, and one named `Car[Hatchback]` is the
//! Hatchback row of `Car`. The name is all there is to it: nothing else is stored, so nothing can
//! disagree with it, and renaming a row moves it. A key can have more than one part, as `Cell[2, 3]`
//! does, which makes the table a grid.
//!
//! A row takes the workspace's own cells it doesn't say from its table, as a node takes them from its
//! kind. Rows of tables that share a key, such as `Car[Hatchback]` and `Petrol car[Hatchback]` where
//! Petrol car is a Car, are parts of one thing: each says some of its cells, and each reads the rest
//! from the others.
//!
//! A row whose key is a number has neighbours, so a formula can read the row before it. A table can
//! say how far it goes, its `Extent`, and then every row in it is there whether or not it's written.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

/// Which row of a table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// `Year[3]`: these come in order.
    Num(i64),
    /// `Car[Hatchback]`: these don't.
    Text(String),
    /// `Cell[2, 3]`: a row of a grid, with a part for each way across it, each a number or text. It
    /// always has at least two parts: one part is just that part.
    Tuple(Vec<Key>),
}

impl Key {
    fn parse(s: &str) -> Option<Key> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return None;
        }
        let one = |p: &str| p.parse().map_or_else(|_| Key::Text(p.to_string()), Key::Num);
        Some(match parts.as_slice() {
            [p] => one(p),
            many => Key::Tuple(many.iter().map(|p| one(p)).collect()),
        })
    }

    /// A key from its parts: one part is that part, and more are a tuple.
    pub fn of(mut parts: Vec<Key>) -> Key {
        if parts.len() == 1 { parts.pop().expect("one part") } else { Key::Tuple(parts) }
    }

    /// Its parts: one, unless it's a tuple.
    pub fn parts(&self) -> Vec<&Key> {
        match self {
            Key::Tuple(parts) => parts.iter().collect(),
            k => vec![k],
        }
    }

    /// How many parts it has: how many ways across its table it goes.
    pub fn arity(&self) -> usize {
        match self {
            Key::Tuple(parts) => parts.len(),
            _ => 1,
        }
    }

    /// Whether every part is a number, so it has neighbours.
    pub fn is_numbered(&self) -> bool {
        self.parts().iter().all(|p| matches!(p, Key::Num(_)))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Num(n) => write!(f, "{n}"),
            Key::Text(s) => f.write_str(s),
            Key::Tuple(parts) => {
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{p}")?;
                }
                Ok(())
            }
        }
    }
}

/// Numbers in order, then the rest by what they say, then tuples part by part. Only a number's order
/// means anything.
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        let rank = |k: &Key| match k {
            Key::Num(_) => 0,
            Key::Text(_) => 1,
            Key::Tuple(_) => 2,
        };
        match (self, other) {
            (Key::Num(a), Key::Num(b)) => a.cmp(b),
            (Key::Text(a), Key::Text(b)) => a.cmp(b),
            (Key::Tuple(a), Key::Tuple(b)) => a.cmp(b),
            (a, b) => rank(a).cmp(&rank(b)),
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Which table a node is a row of, and which row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub table: String,
    pub key: Key,
}

/// The row a name says, if it says one: `Year[3]`, `Petrol car[Hatchback]` or `Cell[2, 3]`. The key is
/// whatever's in the last brackets, split at its commas, with spaces at either end of each part ignored,
/// and the table is everything before them.
pub fn row_of(name: &str) -> Option<Row> {
    let inner = name.trim_end().strip_suffix(']')?;
    let open = inner.rfind('[')?;
    let (table, key) = (inner[..open].trim(), inner[open + 1..].trim());
    if table.is_empty() || key.is_empty() || key.contains(']') {
        return None;
    }
    Some(Row { table: table.to_string(), key: Key::parse(key)? })
}

// ---------- how far a table goes ----------

/// The most rows an extent can say there are, so that going over them all can't run away.
pub const MOST_IN_EXTENT: usize = 100_000;

/// How far a table goes, written on the table as `1..9, 1..9`: a part for each way across it. Inside
/// it, every row is there, written or not; outside it there's none. See `Axis`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extent(pub Vec<Axis>);

/// How far a table goes one way across.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Axis {
    /// `1..9`: the numbers from one to the other, both included. Either end can be `*`, as `1..*`,
    /// which is as far as the rows written go that way.
    Range(End, End),
    /// `[Red, Green, Blue]`: these, in this order.
    Values(Vec<Key>),
    /// `*`: what the rows written say this way, whatever that is.
    Written,
}

/// One end of a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    At(i64),
    /// `*`: as far as the rows written go.
    Written,
}

impl Extent {
    /// `1..9, 1..9`, `*, 1..10`, `[Red, Green], *` or `0..*`.
    pub fn parse(text: &str) -> Result<Extent, String> {
        let mut axes = Vec::new();
        for part in split_top(text)? {
            axes.push(Axis::parse(part)?);
        }
        if axes.is_empty() {
            return Err("an extent needs to say how far it goes, such as 1..9, or 1..9, 1..9 for a grid".to_string());
        }
        let extent = Extent(axes);
        // Only what's fully said can be counted before any rows are written.
        let said: Option<usize> = extent.0.iter().try_fold(1usize, |n, a| match a {
            Axis::Range(End::At(lo), End::At(hi)) => Some(n.saturating_mul(usize::try_from(hi - lo + 1).unwrap_or(0))),
            Axis::Values(v) => Some(n.saturating_mul(v.len())),
            _ => None,
        });
        if said.is_some_and(|n| n > MOST_IN_EXTENT) {
            return Err(format!("{text} is more than {MOST_IN_EXTENT} rows"));
        }
        Ok(extent)
    }

    /// What each way across comes to, given the keys of the rows written: the parts it has, in order.
    /// Rows written with a different number of parts don't count.
    pub fn resolve<'a>(&self, written: impl Iterator<Item = &'a Key>) -> Vec<Vec<Key>> {
        let n = self.0.len();
        let written: Vec<&Key> = written.filter(|k| k.arity() == n).collect();
        self.0
            .iter()
            .enumerate()
            .map(|(i, axis)| {
                let seen = || written.iter().map(|k| k.parts()[i].clone());
                let numbers = || seen().filter_map(|k| if let Key::Num(m) = k { Some(m) } else { None });
                match axis {
                    Axis::Values(v) => v.clone(),
                    Axis::Written => seen().collect::<BTreeSet<Key>>().into_iter().collect(),
                    Axis::Range(lo, hi) => {
                        let lo = match lo {
                            End::At(m) => Some(*m),
                            End::Written => numbers().min(),
                        };
                        let hi = match hi {
                            End::At(m) => Some(*m),
                            End::Written => numbers().max(),
                        };
                        match (lo, hi) {
                            (Some(lo), Some(hi)) if lo <= hi => (lo..=hi).take(MOST_IN_EXTENT).map(Key::Num).collect(),
                            _ => Vec::new(),
                        }
                    }
                }
            })
            .collect()
    }

    /// Whether a row with this key is inside, given what each way across comes to (see `resolve`).
    pub fn contains(axes: &[Vec<Key>], key: &Key) -> bool {
        let parts = key.parts();
        parts.len() == axes.len() && parts.iter().zip(axes).all(|(p, a)| a.contains(p))
    }

    /// Every key inside, in order, given what each way across comes to, up to `MOST_IN_EXTENT`.
    pub fn keys(axes: &[Vec<Key>]) -> Vec<Key> {
        let mut out: Vec<Vec<Key>> = vec![Vec::new()];
        for axis in axes {
            out = out.iter().flat_map(|k| axis.iter().map(move |p| [k.clone(), vec![p.clone()]].concat())).take(MOST_IN_EXTENT).collect();
        }
        if axes.is_empty() {
            return Vec::new();
        }
        out.into_iter().map(Key::of).collect()
    }
}

impl fmt::Display for Extent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, a) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let end = |e: &End| match e {
                End::At(n) => n.to_string(),
                End::Written => "*".to_string(),
            };
            match a {
                Axis::Range(lo, hi) => write!(f, "{}..{}", end(lo), end(hi))?,
                Axis::Written => f.write_str("*")?,
                Axis::Values(v) => write!(f, "[{}]", v.iter().map(Key::to_string).collect::<Vec<_>>().join(", "))?,
            }
        }
        Ok(())
    }
}

impl Axis {
    fn parse(part: &str) -> Result<Axis, String> {
        let part = part.trim();
        if part == "*" {
            return Ok(Axis::Written);
        }
        if let Some(list) = part.strip_prefix('[').and_then(|p| p.strip_suffix(']')) {
            let values: Vec<&str> = list.split(',').map(str::trim).collect();
            if values.iter().any(|v| v.is_empty()) {
                return Err(format!("[{list}] has an empty value in it"));
            }
            let values: Vec<Key> = values.into_iter().map(|v| v.parse().map_or_else(|_| Key::Text(v.to_string()), Key::Num)).collect();
            if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
                return Err(format!("[{list}] says a value twice"));
            }
            return Ok(Axis::Values(values));
        }
        let Some((lo, hi)) = part.split_once("..") else {
            return Err(format!("{part} isn't how far a table goes: that's a range such as 1..9 or 1..*, a list such as [Red, Green], or * for the rows written"));
        };
        let end = |s: &str| -> Result<End, String> {
            match s.trim() {
                "*" => Ok(End::Written),
                n => n.parse().map(End::At).map_err(|_| format!("{n} in {part} isn't a whole number or *")),
            }
        };
        let (lo, hi) = (end(lo)?, end(hi)?);
        if let (End::At(a), End::At(b)) = (lo, hi)
            && a > b
        {
            return Err(format!("{part} goes backwards"));
        }
        Ok(Axis::Range(lo, hi))
    }
}

/// Split at the commas that aren't inside brackets.
fn split_top(text: &str) -> Result<Vec<&str>, String> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        if depth < 0 {
            return Err(format!("{text} has a ] with no [ before it"));
        }
    }
    if depth != 0 {
        return Err(format!("{text} has a [ that isn't closed"));
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    out.push(&text[start..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: i64) -> Key {
        Key::Num(v)
    }

    #[test]
    fn a_name_says_which_row() {
        assert_eq!(row_of("Year[3]"), Some(Row { table: "Year".to_string(), key: Key::Num(3) }));
        assert_eq!(row_of("Year[-1]"), Some(Row { table: "Year".to_string(), key: Key::Num(-1) }));
        assert_eq!(row_of("Petrol car [ Light use ]"), Some(Row { table: "Petrol car".to_string(), key: Key::Text("Light use".to_string()) }));
        assert_eq!(row_of("A[1][2]"), Some(Row { table: "A[1]".to_string(), key: Key::Num(2) }));
        for not in ["Year", "[3]", "Year[]", "Year[3", "Year]3[", "Year[3] and more", "Cell[1,]", "Cell[,1]"] {
            assert_eq!(row_of(not), None, "{not}");
        }
        assert!(Key::Num(10) > Key::Num(9) && Key::Num(10) < Key::Text("a".to_string()));
    }

    #[test]
    fn a_key_can_have_more_than_one_part() {
        assert_eq!(row_of("Cell[2, 3]"), Some(Row { table: "Cell".to_string(), key: Key::Tuple(vec![n(2), n(3)]) }));
        assert_eq!(row_of("Cell[2,3]").unwrap().key, row_of("Cell[ 2 , 3 ]").unwrap().key, "however it's spaced");
        assert_eq!(row_of("Score[Reds, 4]").unwrap().key, Key::Tuple(vec![Key::Text("Reds".to_string()), n(4)]));
        assert_eq!(Key::Tuple(vec![n(2), n(3)]).to_string(), "2, 3");
        assert!(Key::Tuple(vec![n(1), n(9)]) < Key::Tuple(vec![n(2), n(1)]), "part by part");
        assert_eq!(Key::of(vec![n(4)]), n(4), "one part is just that part");
    }

    #[test]
    fn an_extent_says_how_far_a_table_goes() {
        let e = Extent::parse("1..3, [a, b]").unwrap();
        let axes = e.resolve(std::iter::empty());
        assert_eq!(Extent::keys(&axes).len(), 6);
        assert!(Extent::contains(&axes, &Key::Tuple(vec![n(2), Key::Text("b".to_string())])));
        assert!(!Extent::contains(&axes, &Key::Tuple(vec![n(4), Key::Text("b".to_string())])));
        assert!(!Extent::contains(&axes, &n(2)), "a key with the wrong number of parts isn't inside");

        // `*` is what's written that way, and a range can end there.
        let written = [Key::Tuple(vec![Key::Text("Reds".to_string()), n(1)]), Key::Tuple(vec![Key::Text("Blues".to_string()), n(7)])];
        let axes = Extent::parse("*, 1..*").unwrap().resolve(written.iter());
        assert_eq!(axes[0], vec![Key::Text("Blues".to_string()), Key::Text("Reds".to_string())]);
        assert_eq!(axes[1].len(), 7);
        assert_eq!(Extent::parse(" 0..* ").unwrap().resolve(std::iter::empty()), vec![Vec::<Key>::new()], "nothing written, nothing there");

        for (bad, says) in [("", "needs to say"), ("9..1", "backwards"), ("1..x", "whole number"), ("red", "isn't how far"), ("[a, a]", "twice"), ("[a", "isn't closed"), ("0..1000000", "more than")] {
            let err = Extent::parse(bad).unwrap_err();
            assert!(err.contains(says), "{bad}: {err}");
        }
        assert_eq!(Extent::parse("1..9,1..9").unwrap().to_string(), "1..9, 1..9");
    }
}
