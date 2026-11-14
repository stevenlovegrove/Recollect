//! A workspace, and everything the app and Claude can ask of it, with no I/O of its own. A host (the
//! server, or a page in a browser) reads a workspace's files, hands them to `Workspace::load`, and
//! after each change writes the files `Workspace::take_writes` says to. When files change underneath
//! it, `Workspace::files_changed` brings the difference in as an edit.
//!
//! A workspace is files at paths relative to its root, with `/` between folders:
//!
//! ```text
//! recollect.toml       its name, and which systems it uses
//! nodes/<id>.yaml      a node each, with its edges
//! ```

pub mod dispatch;
pub mod ops;
pub mod views;
pub mod workspace;

pub use dispatch::{Effect, METHODS, dispatch};
pub use workspace::{Changed, Config, Error, Files, SAMPLE_FORMAT, SAMPLE_MARKER, Workspace, Writes, new_files, sample_files};
