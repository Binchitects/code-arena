//! A project's key in Arena: its repository's `origin` remote (`host/group/name`), so every clone of it shares its
//! memories; a folder with no remote has a key of this machine's (`local/NAME-HASH`).

use std::path::{Path, PathBuf};

/// `git@gitlab.corp:payments/api.git`, `https://user@gitlab.corp/payments/api`, `ssh://git@gitlab.corp:2222/payments/api.git`
/// → `gitlab.corp/payments/api`.
pub fn from_remote(url: &str) -> Option<String> {
    let u = url.trim();
    let (host, path) = if let Some((scheme, rest)) = u.split_once("://") {
        if !matches!(scheme, "http" | "https" | "ssh" | "git") {
            return None;
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        (host.to_string(), path.to_string())
    } else {
        // scp-like: [user@]host:path
        let (left, path) = u.split_once(':')?;
        if left.contains('/') {
            return None;
        }
        let host = left.rsplit('@').next().unwrap_or(left);
        (host.to_string(), path.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| format!("{}/{path}", host.to_ascii_lowercase()))
}

/// The repository's top folder: the nearest with a `.git` (a folder or a worktree's file).
pub fn git_root(folder: &Path) -> Option<PathBuf> {
    folder.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// The git folder that holds the repository's config: `.git`, or for a worktree its common directory.
fn config_of(top: &Path) -> Option<PathBuf> {
    let git = top.join(".git");
    if git.is_dir() {
        return Some(git.join("config"));
    }
    let pointer = std::fs::read_to_string(&git).ok()?;
    let dir = PathBuf::from(pointer.trim().strip_prefix("gitdir:")?.trim());
    let dir = if dir.is_absolute() { dir } else { top.join(dir) };
    let common = std::fs::read_to_string(dir.join("commondir")).ok().map(|c| {
        let c = PathBuf::from(c.trim());
        if c.is_absolute() { c } else { dir.join(c) }
    });
    Some(common.unwrap_or(dir).join("config"))
}

/// The `origin` remote's URL, from the repository's config.
pub fn origin(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_of(&git_root(root)?)?).ok()?;
    let mut in_origin = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if in_origin
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == "url"
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// A folder's name and a hash of its whole path: two folders of one name stay apart.
pub fn local_key(root: &Path) -> String {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "root".into());
    let name: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    // FNV-1a: stable across runs and versions (the std hasher is not).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{name}-{:08x}", h & 0xffff_ffff)
}

/// The project's key in Arena.
pub fn project_key(root: &Path) -> String {
    origin(root).and_then(|u| from_remote(&u)).unwrap_or_else(|| {
        let top = git_root(root).unwrap_or_else(|| root.to_path_buf());
        format!("local/{}", local_key(&top))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_as_people_write_them() {
        for (url, key) in [
            ("git@gitlab.corp:payments/api.git", "gitlab.corp/payments/api"),
            ("https://user@GitLab.corp/payments/api", "gitlab.corp/payments/api"),
            ("ssh://git@gitlab.corp:2222/payments/api.git", "gitlab.corp/payments/api"),
            ("https://github.com/Binchitects/code-arena.git", "github.com/Binchitects/code-arena"),
        ] {
            assert_eq!(from_remote(url).as_deref(), Some(key), "{url}");
        }
        assert_eq!(from_remote("/srv/repos/api"), None);
        assert_eq!(from_remote("ftp://x/y"), None);
    }

    #[test]
    fn a_repositorys_key_from_its_origin_else_this_machines() {
        let d = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(d.path().join(".git")).expect("git");
        std::fs::create_dir_all(d.path().join("src")).expect("src");
        assert!(project_key(&d.path().join("src")).starts_with("local/"));
        std::fs::write(
            d.path().join(".git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@x:a/b.git\n[remote \"origin\"]\n\turl = git@gitlab.corp:payments/api.git\n",
        )
        .expect("config");
        assert_eq!(project_key(&d.path().join("src")), "gitlab.corp/payments/api");
    }
}
