//! Write the sample workspaces, through a real log: `cargo run -p recollect-systems --example write_samples -- examples`.

use std::path::PathBuf;

use recollect_graph::Log;
use recollect_systems::samples::{SAMPLES, config_text, ops_of};

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("say where to write them"));
    for s in SAMPLES {
        let dir = root.join(s.name);
        assert!(!dir.exists(), "{} is already there: delete it to write it again", dir.display());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("recollect.toml"), config_text(s)).unwrap();
        let names: Vec<String> = s.systems.iter().map(|n| n.to_string()).collect();
        let built = recollect_systems::build(&names).unwrap();
        let (mut log, problems) = Log::open(&dir, built.registry, "sample").unwrap();
        assert_eq!(problems, vec![], "{}", s.name);
        log.commit("sample", s.about, ops_of(s)).unwrap_or_else(|e| panic!("{}: {e:?}", s.name));
        println!("{}: {} nodes", s.name, log.graph().entities().count());
    }
}
