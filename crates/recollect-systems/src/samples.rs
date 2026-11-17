//! The example problems, as workspaces. Each is the files in `examples/<name>/` at the top of the
//! repository, written as any workspace is, and `examples/samples.toml` lists them, in the order the app
//! shows them. `build.rs` embeds every file, so a sample needs nothing from disk wherever it's opened, a
//! browser included, and `tests/samples.rs` checks each still means what it should.

use recollect_graph::SavedGraph;

pub struct Sample {
    pub name: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    /// What kind of thing it is, for finding it among the others.
    pub group: &'static str,
    pub systems: &'static [&'static str],
    /// Its files, by their path inside the workspace, as `recollect.toml` or `nodes/<id>.yaml`.
    pub files: &'static [(&'static str, &'static str)],
}

include!(concat!(env!("OUT_DIR"), "/samples.rs"));

impl Sample {
    /// Its nodes, as saved.
    pub fn saved(&self) -> SavedGraph {
        SavedGraph::from_files(self.files.iter().copied()).unwrap_or_else(|(path, e)| panic!("examples/{}/{path} isn't a valid node: {e}", self.name))
    }
}

/// The sample with this name.
pub fn sample(name: &str) -> Option<&'static Sample> {
    SAMPLES.iter().find(|s| s.name == name)
}
