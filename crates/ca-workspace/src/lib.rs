//! The folder Code Arena works in: where its repository's root is, what the tools may touch, and the project's
//! `.code-arena/` folder, made with its default files the first time.

mod paths;
mod project;

pub use paths::{Outside, Workspace};
pub use project::{Created, ensure_project_folder};
