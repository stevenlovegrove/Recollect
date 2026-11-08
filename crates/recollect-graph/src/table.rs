//! A node named `Year[3]` is row 3 of the table `Year`, and one named `Car[Hatchback]` is the
//! Hatchback row of `Car`. The name is all there is to it: nothing else is stored, so nothing can
//! disagree with it, and renaming a row moves it.
//!
//! A row takes the workspace's own cells it doesn't say from its table, as a node takes them from its
//! kind. Rows of tables that share a key, such as `Car[Hatchback]` and `Petrol car[Hatchback]` where
//! Petrol car is a Car, are parts of one thing: each says some of its cells, and each reads the rest
//! from the others.
//!
//! A row whose key is a number has neighbours, so a formula can read the row before it.

use std::cmp::Ordering;
use std::fmt;

/// Which row of a table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// `Year[3]`: these come in order.
    Num(i64),
    /// `Car[Hatchback]`: these don't.
    Text(String),
}

impl Key {
    fn parse(s: &str) -> Key {
        s.parse().map_or_else(|_| Key::Text(s.to_string()), Key::Num)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Num(n) => write!(f, "{n}"),
            Key::Text(s) => f.write_str(s),
        }
    }
}

/// Numbers in order, then the rest by what they say. Only a number's order means anything.
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Key::Num(a), Key::Num(b)) => a.cmp(b),
            (Key::Num(_), Key::Text(_)) => Ordering::Less,
            (Key::Text(_), Key::Num(_)) => Ordering::Greater,
            (Key::Text(a), Key::Text(b)) => a.cmp(b),
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

/// The row a name says, if it says one: `Year[3]`, or `Petrol car[Hatchback]`. The key is whatever's
/// in the last brackets, with spaces at either end ignored, and the table is everything before them.
pub fn row_of(name: &str) -> Option<Row> {
    let inner = name.trim_end().strip_suffix(']')?;
    let open = inner.rfind('[')?;
    let (table, key) = (inner[..open].trim(), inner[open + 1..].trim());
    if table.is_empty() || key.is_empty() || key.contains(']') {
        return None;
    }
    Some(Row { table: table.to_string(), key: Key::parse(key) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_says_which_row() {
        assert_eq!(row_of("Year[3]"), Some(Row { table: "Year".to_string(), key: Key::Num(3) }));
        assert_eq!(row_of("Year[-1]"), Some(Row { table: "Year".to_string(), key: Key::Num(-1) }));
        assert_eq!(row_of("Petrol car [ Light use ]"), Some(Row { table: "Petrol car".to_string(), key: Key::Text("Light use".to_string()) }));
        assert_eq!(row_of("A[1][2]"), Some(Row { table: "A[1]".to_string(), key: Key::Num(2) }));
        for not in ["Year", "[3]", "Year[]", "Year[3", "Year]3[", "Year[3] and more"] {
            assert_eq!(row_of(not), None, "{not}");
        }
        assert!(Key::Num(10) > Key::Num(9) && Key::Num(10) < Key::Text("a".to_string()));
    }
}
