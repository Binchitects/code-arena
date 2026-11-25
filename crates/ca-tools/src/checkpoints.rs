//! A checkpoint per turn: what each file the turn's tools wrote held before the turn first wrote it (or that it
//! was not there). `/rewind` puts the files back from them. The last 50 turns are kept, in memory, for this run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many turns keep their checkpoint.
const KEPT: usize = 50;

#[derive(Debug, Default)]
pub struct Checkpoints {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// The turn writes go to now: its question's place in the session.
    current: Option<i64>,
    /// By the question's place: each file as it was before that turn wrote it (`None`: it was not there).
    turns: BTreeMap<i64, BTreeMap<PathBuf, Option<Vec<u8>>>>,
}

/// What a rewind of the code did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    pub put_back: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

impl Checkpoints {
    /// A turn begins: its question is at `seq` in the session.
    pub fn begin(&self, seq: i64) {
        if let Ok(mut g) = self.inner.lock() {
            g.current = Some(seq);
            g.turns.entry(seq).or_default();
            while g.turns.len() > KEPT {
                let first = *g.turns.keys().next().unwrap_or(&seq);
                g.turns.remove(&first);
            }
        }
    }

    /// Before a tool writes `path`: what it holds now is kept, once per turn.
    pub fn before_write(&self, path: &Path) {
        let Ok(mut g) = self.inner.lock() else { return };
        let Some(cur) = g.current else { return };
        let turn = g.turns.entry(cur).or_default();
        if !turn.contains_key(path) {
            let before = std::fs::read(path).ok();
            turn.insert(path.to_path_buf(), before);
        }
    }

    /// Whether the turn at `seq` (or a later one) has a checkpoint in this run.
    pub fn has(&self, seq: i64) -> bool {
        self.inner.lock().map(|g| g.turns.contains_key(&seq)).unwrap_or(false)
    }

    /// The files that would change going back to before the turn at `seq`: each with whether it would be
    /// removed (a turn made it).
    pub fn preview(&self, seq: i64) -> Vec<(PathBuf, bool)> {
        let Ok(g) = self.inner.lock() else { return vec![] };
        let mut out: BTreeMap<PathBuf, bool> = BTreeMap::new();
        for (_, files) in g.turns.range(seq..).rev() {
            for (p, before) in files {
                out.insert(p.clone(), before.is_none());
            }
        }
        out.into_iter().collect()
    }

    /// Puts every file back as it was before the turn at `seq`; the checkpoints of that turn and those after it go.
    pub fn rewind(&self, seq: i64) -> Restored {
        let Ok(mut g) = self.inner.lock() else { return Restored::default() };
        let mut first_state: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
        // The earliest turn's "before" is what the file held before all of them.
        for (_, files) in g.turns.range(seq..).rev() {
            for (p, before) in files {
                first_state.insert(p.clone(), before.clone());
            }
        }
        let later: Vec<i64> = g.turns.range(seq..).map(|(k, _)| *k).collect();
        for k in later {
            g.turns.remove(&k);
        }
        g.current = None;
        let mut r = Restored::default();
        for (p, before) in first_state {
            match before {
                Some(bytes) => match std::fs::write(&p, bytes) {
                    Ok(()) => r.put_back.push(p),
                    Err(e) => r.failed.push((p, e.to_string())),
                },
                None => match std::fs::remove_file(&p) {
                    Ok(()) => r.removed.push(p),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => r.failed.push((p, e.to_string())),
                },
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_go_back_as_they_were_before_the_turn() {
        let d = tempfile::tempdir().expect("temp");
        let a = d.path().join("a.txt");
        let made = d.path().join("made.txt");
        std::fs::write(&a, "one").expect("w");
        let c = Checkpoints::default();
        c.begin(0);
        c.before_write(&a);
        std::fs::write(&a, "two").expect("w");
        c.before_write(&a);
        std::fs::write(&a, "three").expect("w");
        c.begin(4);
        c.before_write(&a);
        std::fs::write(&a, "four").expect("w");
        c.before_write(&made);
        std::fs::write(&made, "new").expect("w");
        assert_eq!(c.preview(4), vec![(a.clone(), false), (made.clone(), true)]);
        let r = c.rewind(4);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "three");
        assert!(!made.exists());
        assert_eq!((r.put_back.len(), r.removed.len()), (1, 1));
        assert!(!c.has(4) && c.has(0));
        c.rewind(0);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "one");
    }
}
