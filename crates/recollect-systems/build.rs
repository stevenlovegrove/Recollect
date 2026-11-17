//! The sample workspaces are the files in `examples/` at the top of the repository, as anyone would
//! write them. This embeds them, so whatever's built from this crate, the server or the wasm in a
//! browser, has every sample with nothing to read from disk. `examples/samples.toml` says which there
//! are, in what order, and what kind of thing each is; each one's `recollect.toml` says its title, what
//! it's about and its systems.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct Index {
    sample: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Entry {
    name: String,
    group: String,
}

#[derive(serde::Deserialize)]
struct Config {
    name: String,
    #[serde(default)]
    about: String,
    #[serde(default)]
    systems: Vec<String>,
}

/// Every file under `dir`, by its path from there, with `/` between parts.
fn files(dir: &Path, under: &str, out: &mut BTreeMap<String, PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let at = if under.is_empty() { name } else { format!("{under}/{name}") };
        if path.is_dir() {
            files(&path, &at, out);
        } else {
            out.insert(at, path);
        }
    }
}

/// FNV-1a: enough to tell whether any sample has changed, with nothing to depend on.
fn hash(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").canonicalize().expect("the examples are at the top of the repository");
    println!("cargo:rerun-if-changed={}", root.display());
    let index: Index = toml::from_str(&std::fs::read_to_string(root.join("samples.toml")).unwrap()).unwrap_or_else(|e| panic!("examples/samples.toml: {e}"));

    // Every sample listed is there, and every one there is listed.
    let listed: Vec<&str> = index.sample.iter().map(|e| e.name.as_str()).collect();
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() && !listed.contains(&name.as_str()) {
            panic!("examples/{name} isn't listed in examples/samples.toml");
        }
    }

    let mut format = 0xcbf29ce484222325u64;
    let mut out = String::from("/// The sample workspaces, from `examples/`: see `build.rs`.\npub const SAMPLES: &[Sample] = &[\n");
    for e in &index.sample {
        let dir = root.join(&e.name);
        let config: Config = toml::from_str(&std::fs::read_to_string(dir.join("recollect.toml")).unwrap_or_else(|err| panic!("examples/{}: {err}", e.name))).unwrap_or_else(|err| panic!("examples/{}/recollect.toml: {err}", e.name));
        let mut found = BTreeMap::new();
        files(&dir, "", &mut found);
        let systems: Vec<String> = config.systems.iter().map(|s| format!("{s:?}")).collect();
        write!(out, "    Sample {{ name: {:?}, title: {:?}, about: {:?}, group: {:?}, systems: &[{}], files: &[", e.name, config.name, config.about, e.group, systems.join(", ")).unwrap();
        for (at, path) in &found {
            hash(&mut format, format!("{}/{at}", e.name).as_bytes());
            hash(&mut format, &std::fs::read(path).unwrap());
            write!(out, "({at:?}, include_str!({:?})), ", path.display().to_string()).unwrap();
        }
        out.push_str("] },\n");
    }
    out.push_str("];\n\n");
    writeln!(out, "/// Changes whenever any sample does, so an old working copy is made again, not misread.\npub const FORMAT: &str = \"{format:016x}\";").unwrap();
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("samples.rs"), out).unwrap();
}
