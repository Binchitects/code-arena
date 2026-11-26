//! The program's log: what Code Arena says while it runs, kept in a file (timed lines, no key in them) so that
//! a problem can be looked into afterwards. Nothing is kept yet: only where the log is, for the About box.

use std::path::PathBuf;

/// Where the log is written; `None` while none is kept.
pub fn path() -> Option<PathBuf> {
    None
}
