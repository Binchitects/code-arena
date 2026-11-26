//! Skills brought in and taken out as files: a `.zip` (one skill's folder, or several), a folder (one skill's, with its
//! SKILL.md, or one holding several), or a lone SKILL.md. Within limits (50 skills, 5 MB); every file inside its
//! skill's folder; a file that looks like it holds a secret (a key, a token, a private key) never comes in, nor goes out.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MOST_SKILLS: usize = 50;
pub const MOST_BYTES: u64 = 5 * 1024 * 1024;
pub const SKILL_MD: &str = "SKILL.md";

/// One skill's folder: its name, and each text file by its path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub files: BTreeMap<String, String>,
}

/// What was read: the skills, and what was left out and why.
#[derive(Debug, Default)]
pub struct Found {
    pub skills: Vec<Folder>,
    pub left_out: Vec<String>,
}

/// Whether a text looks like it holds a secret: what never comes in.
pub fn secret_in(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY") {
        return Some("a private key");
    }
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ':' | ',' | '`')) {
        let w = word.trim();
        let long = |prefix: &str, n: usize| w.starts_with(prefix) && w.len() >= prefix.len() + n;
        if long("sk-", 20) || long("sk-ant-", 20) {
            return Some("an API key (sk-…)");
        }
        if long("ghp_", 30) || long("github_pat_", 30) || long("glpat-", 18) {
            return Some("a token for GitHub or GitLab");
        }
        if long("xoxb-", 20) || long("xoxp-", 20) {
            return Some("a Slack token");
        }
        if w.len() == 20 && w.starts_with("AKIA") && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Some("an AWS access key");
        }
    }
    None
}

fn clean(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./");
    let parts: Vec<&str> = p.split('/').collect();
    if p.is_empty()
        || p.starts_with('/')
        || parts.iter().any(|x| x.is_empty() || *x == "." || *x == "..")
        || p.contains(':')
    {
        return None;
    }
    Some(p.to_string())
}

/// A skill's name from its SKILL.md's front matter, else from where it was found.
fn name_of(files: &BTreeMap<String, String>, fallback: &str) -> String {
    let fm = files.get(SKILL_MD).map(|t| crate::library::front_matter(t).0).unwrap_or_default();
    let n = fm.get("name").cloned().unwrap_or_else(|| fallback.to_string());
    n.trim().to_lowercase().replace(' ', "-")
}

/// Every file of a tree, by its path inside `root` (text files only).
fn tree(root: &Path, out: &mut BTreeMap<String, Vec<u8>>, left_out: &mut Vec<String>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if e.file_name() != ".git" {
                    stack.push(p);
                }
                continue;
            }
            let rel = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            match std::fs::read(&p) {
                Ok(b) => {
                    out.insert(rel, b);
                }
                Err(e) => left_out.push(format!("{rel}: {e}")),
            }
        }
    }
}

/// The files read (a zip's entries, or a folder's), grouped into skills: those at the top (a SKILL.md there: one skill)
/// or one folder down.
fn group(all: BTreeMap<String, Vec<u8>>, top_name: &str) -> Result<Found, String> {
    let total: u64 = all.values().map(|b| b.len() as u64).sum();
    if total > MOST_BYTES {
        return Err(format!(
            "An import is at most {} MB; this is {:.1} MB.",
            MOST_BYTES / 1024 / 1024,
            total as f64 / 1048576.0
        ));
    }
    let mut r = Found::default();
    let mut text: BTreeMap<String, String> = BTreeMap::new();
    for (path, bytes) in all {
        let Some(p) = clean(&path) else {
            r.left_out.push(format!("{path}: outside its skill's folder"));
            continue;
        };
        match String::from_utf8(bytes) {
            Ok(t) => match secret_in(&t) {
                Some(what) => r.left_out.push(format!("{p}: it looks like it holds {what}, and secrets never come in")),
                None => {
                    text.insert(p, t);
                }
            },
            Err(_) => r.left_out.push(format!("{p}: not a text file")),
        }
    }
    // A zip of one folder: its entries under that folder's name.
    let tops: std::collections::BTreeSet<&str> = text.keys().map(|k| k.split('/').next().unwrap_or("")).collect();
    let single_dir = tops.len() == 1 && !text.contains_key(SKILL_MD) && text.keys().all(|k| k.contains('/'));
    let (text, top_name) = if single_dir {
        let dir = tops.iter().next().map(|s| s.to_string()).unwrap_or_default();
        let inner: BTreeMap<String, String> = text
            .into_iter()
            .filter_map(|(k, v)| k.strip_prefix(&format!("{dir}/")).map(|k| (k.to_string(), v)))
            .collect();
        (inner, dir)
    } else {
        (text, top_name.to_string())
    };
    if text.contains_key(SKILL_MD) {
        let name = name_of(&text, &top_name);
        r.skills.push(Folder { name, files: text });
    } else {
        let mut by: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (k, v) in text {
            let Some((dir, rest)) = k.split_once('/') else {
                r.left_out.push(format!("{k}: not in a skill's folder"));
                continue;
            };
            by.entry(dir.to_string()).or_default().insert(rest.to_string(), v);
        }
        for (dir, files) in by {
            if !files.contains_key(SKILL_MD) {
                r.left_out.push(format!("{dir}/: no SKILL.md"));
                continue;
            }
            let name = name_of(&files, &dir);
            r.skills.push(Folder { name, files });
        }
    }
    r.skills.retain(|f| {
        let ok = crate::library::good_skill_name(&f.name);
        if !ok {
            r.left_out.push(format!("{}: not a skill's name (lower-case letters, digits, - and _)", f.name));
        }
        ok
    });
    if r.skills.len() > MOST_SKILLS {
        return Err(format!("An import brings at most {MOST_SKILLS} skills; this has {}.", r.skills.len()));
    }
    if r.skills.is_empty() {
        return Err(format!(
            "No skill in it: a skill is a folder with its SKILL.md{}.",
            if r.left_out.is_empty() { String::new() } else { format!(" (left out: {})", r.left_out.join("; ")) }
        ));
    }
    Ok(r)
}

/// The skills in a `.zip`, a folder or a SKILL.md.
pub fn read(path: &Path) -> Result<Found, String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if path.is_dir() {
        let mut all = BTreeMap::new();
        let mut left = Vec::new();
        tree(path, &mut all, &mut left);
        let mut r = group(all, &stem)?;
        r.left_out.extend(left);
        return Ok(r);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.file_name().is_some_and(|n| n == SKILL_MD) {
        let dir = path.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or(stem);
        return group(BTreeMap::from([(SKILL_MD.to_string(), bytes)]), &dir);
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("{} is not a zip: {e}", path.display()))?;
    let mut all = BTreeMap::new();
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        total += f.size();
        if total > MOST_BYTES {
            return Err(format!("An import is at most {} MB.", MOST_BYTES / 1024 / 1024));
        }
        let name = f.name().to_string();
        if name.starts_with("__MACOSX/") || name.ends_with(".DS_Store") {
            continue;
        }
        let mut b = Vec::new();
        // At most what the size says, whatever the entry claims after.
        f.by_ref().take(MOST_BYTES).read_to_end(&mut b).map_err(|e| format!("{name}: {e}"))?;
        all.insert(name, b);
    }
    group(all, &stem)
}

/// Writes skills into a skills folder (`.code-arena/skills`, the config folder's `skills`): one there already is kept
/// unless `replace`. Gives what was written and what was kept.
pub fn write_into(dir: &Path, skills: &[Folder], replace: bool) -> std::io::Result<(Vec<String>, Vec<String>)> {
    let (mut done, mut kept) = (Vec::new(), Vec::new());
    for s in skills {
        let target = dir.join(&s.name);
        if target.exists() && !replace {
            kept.push(s.name.clone());
            continue;
        }
        let _ = std::fs::remove_dir_all(&target);
        for (rel, text) in &s.files {
            let f = target.join(rel);
            if let Some(d) = f.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(f, text)?;
        }
        done.push(s.name.clone());
    }
    Ok((done, kept))
}

/// A skill's folder (where its SKILL.md is) as a `.zip` at `to`, its folder's name at the top; a file that looks like it
/// holds a secret is left out. Gives what was left out.
pub fn export_zip(skill_dir: &Path, name: &str, to: &Path) -> Result<Vec<String>, String> {
    let mut all = BTreeMap::new();
    let mut left = Vec::new();
    tree(skill_dir, &mut all, &mut left);
    let file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (rel, bytes) in all {
        if let Ok(t) = std::str::from_utf8(&bytes)
            && let Some(what) = secret_in(t)
        {
            left.push(format!("{rel}: it looks like it holds {what}"));
            continue;
        }
        z.start_file(format!("{name}/{rel}"), opts).map_err(|e| e.to_string())?;
        z.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(left)
}

/// Where an export goes when no place is given: `NAME.zip` in `cwd`.
pub fn default_export(cwd: &Path, name: &str) -> PathBuf {
    cwd.join(format!("{name}.zip"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir.join(name)).expect("mkdir");
        std::fs::write(dir.join(name).join(SKILL_MD), format!("---\nname: {name}\ndescription: d\n---\n{body}"))
            .expect("w");
    }

    #[test]
    fn a_folder_of_skills_a_lone_skill_md_and_a_zip_round_trip() {
        let d = tempfile::tempdir().expect("temp");
        let many = d.path().join("many");
        skill(&many, "release", "Tag it.");
        skill(&many, "review", "Look.");
        std::fs::write(many.join("release/notes.md"), "Notes.").expect("w");
        std::fs::create_dir_all(many.join("stray")).expect("mkdir");
        std::fs::write(many.join("stray/readme.md"), "no skill here").expect("w");
        let r = read(&many).expect("read");
        assert_eq!(r.skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["release", "review"]);
        assert!(r.left_out.iter().any(|l| l.contains("stray/: no SKILL.md")));
        let one = read(&many.join("release")).expect("one");
        assert_eq!(one.skills[0].files.keys().collect::<Vec<_>>(), vec!["SKILL.md", "notes.md"]);
        let lone = read(&many.join("review").join(SKILL_MD)).expect("lone");
        assert_eq!(lone.skills[0].name, "review");
        // Out as a zip, and in again.
        let z = d.path().join("release.zip");
        assert!(export_zip(&many.join("release"), "release", &z).expect("zip").is_empty());
        let back = read(&z).expect("unzipped");
        assert_eq!(back.skills[0].name, "release");
        assert_eq!(back.skills[0].files["notes.md"], "Notes.");
        // Written into a skills folder: one there already is kept unless replaced.
        let into = d.path().join("into");
        assert_eq!(write_into(&into, &back.skills, false).expect("w").0, vec!["release"]);
        assert_eq!(write_into(&into, &back.skills, false).expect("w").1, vec!["release"]);
        assert!(into.join("release/notes.md").is_file());
    }

    #[test]
    fn secrets_never_come_in_and_files_stay_in_their_folder() {
        let d = tempfile::tempdir().expect("temp");
        let s = d.path().join("deploy");
        skill(d.path(), "deploy", "Deploy it.");
        std::fs::write(s.join("env.md"), "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz123456").expect("w");
        std::fs::write(s.join("key.pem"), "-----BEGIN RSA PRIVATE KEY-----\nxx\n-----END RSA PRIVATE KEY-----")
            .expect("w");
        let r = read(&s).expect("read");
        assert_eq!(r.skills[0].files.keys().collect::<Vec<_>>(), vec!["SKILL.md"]);
        assert_eq!(r.left_out.len(), 2, "{:?}", r.left_out);
        assert!(secret_in("glpat-abcdefghijklmnopqrstu").is_some());
        assert!(secret_in("the sk- prefix and AKIA are words here").is_none());
        // A zip whose entry climbs out of its folder.
        let z = d.path().join("bad.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).expect("f"));
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("ok/SKILL.md", o).expect("s");
            w.write_all(b"---\nname: ok\n---\nx").expect("w");
            w.start_file("ok/../../escape.md", o).expect("s");
            w.write_all(b"no").expect("w");
            w.finish().expect("finish");
        }
        let r = read(&z).expect("read");
        assert_eq!(r.skills[0].name, "ok");
        assert!(r.left_out.iter().any(|l| l.contains("outside")), "{:?}", r.left_out);
    }
}
