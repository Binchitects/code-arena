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
    /// The plan's steps in the order they began in this run: each file as it was before that step wrote it.
    steps: Vec<(u32, Before)>,
    /// What a file the person accepted the agent's changes to is compared with from then on: as it was when
    /// accepted (or the text accepted of it). The turns and the steps keep their own copies, for `/rewind` and
    /// `/plan undo`.
    accepted: Before,
}

/// Each file as it was before something wrote it (`None`: it was not there).
type Before = BTreeMap<PathBuf, Option<Vec<u8>>>;

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

    /// Before a tool writes `path`: what it holds now is kept, once per turn and once per plan step.
    pub fn before_write(&self, path: &Path) {
        let Ok(mut g) = self.inner.lock() else { return };
        let before = std::fs::read(path).ok();
        if let Some((_, step)) = g.steps.last_mut()
            && !step.contains_key(path)
        {
            step.insert(path.to_path_buf(), before.clone());
        }
        let Some(cur) = g.current else { return };
        let turn = g.turns.entry(cur).or_default();
        if !turn.contains_key(path) {
            turn.insert(path.to_path_buf(), before);
        }
    }

    /// A step of the plan began: what its writes change is kept from now, for [`undo_step`](Self::undo_step).
    pub fn begin_step(&self, number: u32) {
        if let Ok(mut g) = self.inner.lock() {
            g.steps.retain(|(n, _)| *n != number);
            g.steps.push((number, BTreeMap::new()));
        }
    }

    /// The steps begun in this run, in order.
    pub fn steps(&self) -> Vec<u32> {
        self.inner.lock().map(|g| g.steps.iter().map(|(n, _)| *n).collect()).unwrap_or_default()
    }

    /// Puts the files back as they were before step `number` began, and before every step begun after it. Gives the
    /// steps undone and what was done; `None` when that step has not begun in this run.
    pub fn undo_step(&self, number: u32) -> Option<(Vec<u32>, Restored)> {
        let Ok(mut g) = self.inner.lock() else { return None };
        let at = g.steps.iter().position(|(n, _)| *n == number)?;
        let undone: Vec<(u32, Before)> = g.steps.drain(at..).collect();
        drop(g);
        let mut first_state: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
        for (_, files) in undone.iter().rev() {
            for (p, before) in files {
                first_state.insert(p.clone(), before.clone());
            }
        }
        // What was accepted of a file put back is undone with it.
        if let Ok(mut g) = self.inner.lock() {
            for p in first_state.keys() {
                g.accepted.remove(p);
            }
        }
        Some((undone.iter().map(|(n, _)| *n).collect(), put_back(first_state)))
    }

    /// Whether the turn at `seq` (or a later one) has a checkpoint in this run.
    pub fn has(&self, seq: i64) -> bool {
        self.inner.lock().map(|g| g.turns.contains_key(&seq)).unwrap_or(false)
    }

    /// Each file the agent wrote in this run, as it was before the first of those writes (`None`: it was not there):
    /// what the agent's changes are compared with.
    pub fn originals(&self) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let Ok(g) = self.inner.lock() else { return vec![] };
        let mut out: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
        // The earliest turn's copy of a file wins: later ones hold what an earlier turn already changed.
        for files in g.turns.values().rev() {
            for (p, before) in files {
                out.insert(p.clone(), before.clone());
            }
        }
        for (p, before) in out.iter_mut() {
            if let Some(kept) = g.accepted.get(p) {
                *before = kept.clone();
            }
        }
        out.into_iter().collect()
    }

    /// What the agent's changes to `path` are compared with; `None` when the agent wrote no such file in this run.
    pub fn original(&self, path: &Path) -> Option<Option<Vec<u8>>> {
        self.originals().into_iter().find(|(p, _)| p == path).map(|(_, before)| before)
    }

    /// The agent's changes to `path` compared from now on with `bytes` (`None`: as a file that was not there), as
    /// when only a part of them was accepted. False when the agent wrote no such file in this run.
    pub fn set_original(&self, path: &Path, bytes: Option<Vec<u8>>) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        if !g.turns.values().any(|t| t.contains_key(path)) {
            return false;
        }
        g.accepted.insert(path.to_path_buf(), bytes);
        true
    }

    /// The agent's changes to `path` kept: it is compared from now on with what it holds now, so it is no longer
    /// a change until the agent writes it again.
    pub fn accept(&self, path: &Path) -> bool {
        self.set_original(path, std::fs::read(path).ok())
    }

    /// Every change the agent made kept (see [`accept`](Self::accept)).
    pub fn accept_all(&self) {
        for (p, _) in self.originals() {
            self.accept(&p);
        }
    }

    /// The agent's changes to `path` undone: the file as it was (or as it was accepted), removed when the agent
    /// made it. `None` when the agent wrote no such file in this run.
    pub fn revert(&self, path: &Path) -> Option<Restored> {
        let before = self.original(path)?;
        Some(put_back(BTreeMap::from([(path.to_path_buf(), before)])))
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
        // What was accepted of a file put back is undone with it.
        for p in first_state.keys() {
            g.accepted.remove(p);
        }
        drop(g);
        put_back(first_state)
    }
}

/// Each file as it was: written back, or removed when it was not there.
fn put_back(first_state: BTreeMap<PathBuf, Option<Vec<u8>>>) -> Restored {
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
        // What the changes are compared with: a.txt as before the first turn, made.txt as not there.
        assert_eq!(c.originals(), vec![(a.clone(), Some(b"one".to_vec())), (made.clone(), None)]);
        let r = c.rewind(4);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "three");
        assert!(!made.exists());
        assert_eq!((r.put_back.len(), r.removed.len()), (1, 1));
        assert!(!c.has(4) && c.has(0));
        c.rewind(0);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "one");
    }

    #[test]
    fn a_step_and_those_begun_after_it_are_undone() {
        let d = tempfile::tempdir().expect("temp");
        let a = d.path().join("a.txt");
        std::fs::write(&a, "start").expect("w");
        let c = Checkpoints::default();
        c.begin(0);
        c.begin_step(1);
        c.before_write(&a);
        std::fs::write(&a, "step 1").expect("w");
        c.begin_step(2);
        let b = d.path().join("b.txt");
        c.before_write(&b);
        std::fs::write(&b, "made in step 2").expect("w");
        c.before_write(&a);
        std::fs::write(&a, "step 2").expect("w");
        assert_eq!(c.steps(), vec![1, 2]);
        let (steps, r) = c.undo_step(2).expect("begun");
        assert_eq!(steps, vec![2]);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "step 1");
        assert!(!b.exists() && r.removed.len() == 1);
        let (steps, _) = c.undo_step(1).expect("begun");
        assert_eq!(steps, vec![1]);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "start");
        assert!(c.undo_step(1).is_none());
    }

    #[test]
    fn an_accepted_file_is_compared_with_what_it_held_then_and_a_revert_puts_it_back_so() {
        let d = tempfile::tempdir().expect("temp");
        let a = d.path().join("a.txt");
        let made = d.path().join("made.txt");
        std::fs::write(&a, "one").expect("w");
        let c = Checkpoints::default();
        c.begin(0);
        c.before_write(&a);
        std::fs::write(&a, "two").expect("w");
        c.before_write(&made);
        std::fs::write(&made, "new").expect("w");
        assert!(c.accept(&a));
        assert_eq!(c.original(&a), Some(Some(b"two".to_vec())));
        assert!(!c.accept(&d.path().join("never.txt")), "a file the agent did not write is not accepted");
        // Written again: a change from the accepted text, which a revert goes back to.
        c.begin(2);
        c.before_write(&a);
        std::fs::write(&a, "three").expect("w");
        let r = c.revert(&a).expect("written");
        assert_eq!((std::fs::read_to_string(&a).expect("r"), r.put_back.len()), ("two".into(), 1));
        // A file the agent made goes when reverted.
        assert_eq!(c.revert(&made).map(|r| r.removed.len()), Some(1));
        assert!(!made.exists());
        // Part accepted: compared with the text given.
        assert!(c.set_original(&a, Some(b"half".to_vec())));
        assert_eq!(c.original(&a), Some(Some(b"half".to_vec())));
        // The rewind still puts the file back as it was before the turn, and forgets what was accepted of it.
        c.rewind(0);
        assert_eq!(std::fs::read_to_string(&a).expect("r"), "one");
        assert_eq!(c.original(&a), None);
    }

    #[test]
    fn accept_all_leaves_no_change() {
        let d = tempfile::tempdir().expect("temp");
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "one").expect("w");
        let c = Checkpoints::default();
        c.begin(0);
        for p in [&a, &b] {
            c.before_write(p);
            std::fs::write(p, "changed").expect("w");
        }
        c.accept_all();
        assert!(c.originals().iter().all(|(p, before)| std::fs::read(p).ok() == *before));
    }
}
