//! The IDE's own settings (the editor's, the workbench's, the Explorer's, the terminal's, ...), declared as the
//! others are in [`crate::schema`] and found by [`crate::schema::find`] with them. A new one is a row added to
//! [`ALL`]. None is declared yet.

use crate::schema::Setting;

/// Every setting of the IDE's, in the Settings editor's order.
pub static ALL: &[&Setting] = &[];
