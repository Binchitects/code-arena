//! Builds the page into the binary: every file of `web/dist` (made by `npm run build` in `web/`), or a page saying it
//! was not built when there is none.
use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest.join("../../web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut found = Vec::new();
    files(&dist, &mut found);
    let mut code = String::from(
        "/// The page's files: their path from the root, and their bytes.\npub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for f in &found {
        let rel = f.strip_prefix(&dist).expect("under dist").to_string_lossy().replace('\\', "/");
        println!("cargo:rerun-if-changed={}", f.display());
        code.push_str(&format!("    ({rel:?}, include_bytes!({:?})),\n", f.canonicalize().expect("real path")));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("page.rs");
    std::fs::write(out, code).expect("page.rs");
}
