//! The panes: one module per centre view, plus the file tree.
//!
//! A pane is a function of the shared state and a `Ui`. Nothing here starts
//! work: a pane reads what the state already holds and asks for a reload when it
//! needs one, which keeps every frame bounded by drawing.

pub mod card;
pub mod conversation;
pub mod editor;
pub mod git;
pub mod observability;
pub mod terminal;
pub mod tree;
pub mod workflows;
