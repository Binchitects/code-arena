//! What the agent's tools may touch: the working directory and the folders added beside it, nothing else. A link
//! is followed as the system follows it, so a link inside that leads outside counts as outside.

use std::path::{Component, Path, PathBuf};

/// A path the tools may not use, and why.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{path} is outside the folders the tools may use ({why})")]
pub struct Outside {
    pub path: String,
    pub why: String,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    /// The working directory, its links resolved.
    cwd: PathBuf,
    /// The repository's root (the folder holding `.git`), else the working directory.
    root: PathBuf,
    /// The working directory and the folders added, links resolved.
    allowed: Vec<PathBuf>,
}

/// More links than this on one path is a loop, as the system says (ELOOP).
const MOST_LINKS: usize = 40;

impl Workspace {
    pub fn open(cwd: &Path, added: &[PathBuf]) -> std::io::Result<Self> {
        let cwd = std::fs::canonicalize(cwd)?;
        let root = repository_root(&cwd).unwrap_or_else(|| cwd.clone());
        let mut allowed = vec![cwd.clone()];
        for a in added {
            let p = if a.is_absolute() { a.clone() } else { cwd.join(a) };
            allowed.push(std::fs::canonicalize(&p)?);
        }
        Ok(Self { cwd, root, allowed })
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The repository's root, else the working directory: where `ARENA.md` and `.code-arena/` are.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn in_repository(&self) -> bool {
        self.root.join(".git").exists()
    }

    pub fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }

    /// A tool's path (relative to the working directory, or whole) as the real path it names, when that is inside.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, Outside> {
        let outside = |why: &str| Outside { path: path.to_string(), why: why.to_string() };
        if path.is_empty() {
            return Err(outside("no path given"));
        }
        if path.contains('\0') {
            return Err(outside("a path cannot hold a NUL"));
        }
        let given = git_bash_path(path);
        let joined = if given.is_absolute() { given } else { self.cwd.join(given) };
        let real = real_path(&joined, 0).map_err(|e| outside(&e))?;
        if self.allowed.iter().any(|a| real.starts_with(a)) { Ok(real) } else { Err(outside("it leads out of them")) }
    }

    /// A real path as people read it: relative to the working directory when inside it.
    pub fn display(&self, real: &Path) -> String {
        match real.strip_prefix(&self.cwd) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => real.to_string_lossy().into_owned(),
        }
    }
}

/// The path as the system resolves it: each link followed where it leads, a `..` after a link applied to where the
/// link led, not to its name. A part that is not there yet is kept by name (a file about to be written).
fn real_path(p: &Path, links: usize) -> Result<PathBuf, String> {
    if links > MOST_LINKS {
        return Err("a loop of links, or too many on the way".into());
    }
    if let Ok(r) = std::fs::canonicalize(p) {
        return Ok(r);
    }
    // Not all of it is there. A dangling link: follow it by hand.
    if let Ok(meta) = std::fs::symlink_metadata(p)
        && meta.file_type().is_symlink()
    {
        let target = std::fs::read_link(p).map_err(|e| e.to_string())?;
        let base = p.parent().unwrap_or(Path::new("/"));
        let next = if target.is_absolute() { target } else { base.join(target) };
        return real_path(&next, links + 1);
    }
    let last = p.components().next_back().ok_or("an empty path")?;
    let parent = p.parent().ok_or("no folder above it")?;
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let base = real_path(parent, links + 1)?;
    Ok(match last {
        Component::ParentDir => base.parent().map(Path::to_path_buf).unwrap_or(base),
        Component::CurDir => base,
        Component::Normal(name) => base.join(name),
        Component::RootDir | Component::Prefix(_) => base,
    })
}

/// The folder holding `.git`, from `dir` up.
fn repository_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// On Windows a path as Git's bash writes it (`/c/Users/me`) is `C:/Users/me`.
fn git_bash_path(path: &str) -> PathBuf {
    if cfg!(windows) {
        let b = path.as_bytes();
        if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
            return PathBuf::from(format!("{}:/{}", b[1] as char, path.get(3..).unwrap_or("")));
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, Workspace, PathBuf) {
        let dir = tempfile::tempdir().expect("temp");
        let inside = dir.path().join("project");
        std::fs::create_dir_all(inside.join("src")).expect("mkdir");
        std::fs::write(inside.join("src/a.rs"), "fn main() {}").expect("write");
        std::fs::write(dir.path().join("secret.txt"), "no").expect("write");
        let w = Workspace::open(&inside, &[]).expect("open");
        let outer = std::fs::canonicalize(dir.path()).expect("canon");
        (dir, w, outer)
    }

    #[test]
    fn relative_and_whole_paths_inside() {
        let (_d, w, outer) = ws();
        assert_eq!(w.resolve("src/a.rs").expect("in"), outer.join("project/src/a.rs"));
        assert_eq!(w.resolve("src/new.rs").expect("in"), outer.join("project/src/new.rs"));
        assert_eq!(w.display(&w.resolve("src/../src/a.rs").expect("in")), "src/a.rs");
        let whole = outer.join("project/src/a.rs");
        assert!(w.resolve(&whole.to_string_lossy()).is_ok());
    }

    #[test]
    fn dot_dot_out_is_outside() {
        let (_d, w, _) = ws();
        assert!(w.resolve("../secret.txt").is_err());
        assert!(w.resolve("src/../../secret.txt").is_err());
        assert!(w.resolve("/etc/passwd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_followed_as_the_system_does() {
        let (_d, w, outer) = ws();
        let p = outer.join("project");
        std::os::unix::fs::symlink(&outer, p.join("up")).expect("link");
        assert!(w.resolve("up/secret.txt").is_err(), "a link inside that leads out");
        // d -> /, x -> d/../etc/passwd is /etc/passwd: the .. applies after the link.
        std::os::unix::fs::symlink("/", p.join("d")).expect("link");
        std::os::unix::fs::symlink("d/../etc/passwd", p.join("x")).expect("link");
        assert!(w.resolve("x").is_err());
        // A dangling link to outside.
        std::os::unix::fs::symlink(outer.join("nothing-here"), p.join("dangling")).expect("link");
        assert!(w.resolve("dangling").is_err());
        // A loop.
        std::os::unix::fs::symlink(p.join("loop2"), p.join("loop1")).expect("link");
        std::os::unix::fs::symlink(p.join("loop1"), p.join("loop2")).expect("link");
        assert!(w.resolve("loop1").is_err());
        // A link that stays inside is fine.
        std::os::unix::fs::symlink("src", p.join("code")).expect("link");
        assert_eq!(w.resolve("code/a.rs").expect("in"), p.join("src/a.rs"));
    }

    #[test]
    fn an_added_folder_is_inside() {
        let (d, _w, outer) = ws();
        let w = Workspace::open(&d.path().join("project"), &[d.path().to_path_buf()]).expect("open");
        assert_eq!(w.resolve("../secret.txt").expect("allowed"), outer.join("secret.txt"));
    }

    #[test]
    fn the_repository_root_is_found() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(dir.path().join(".git")).expect("git");
        std::fs::create_dir_all(dir.path().join("pkg/a")).expect("mkdir");
        let w = Workspace::open(&dir.path().join("pkg/a"), &[]).expect("open");
        assert_eq!(w.root(), std::fs::canonicalize(dir.path()).expect("canon"));
        assert!(w.in_repository());
    }
}
