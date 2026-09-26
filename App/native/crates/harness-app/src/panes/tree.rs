//! The file tree.
//!
//! The tree is built from directory listings the state caches per directory, so
//! expanding a folder reads it once. Rows are flattened before they are drawn:
//! the recursion needs the state mutably and the click handling needs it again,
//! and a flat list of rows keeps those two apart.
//!
//! Ignored paths are hidden by default with a toggle to show them dimmed, which
//! is the behaviour of every tool that has ever walked a `node_modules`.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, TextEdit, Ui};

use crate::state::{HarnessState, TreeState};
use crate::theme;

/// The most rows a tree draws, so a huge directory cannot stall a frame.
const TREE_ROWS: usize = 6_000;

/// One visible row.
struct Row {
    /// Absolute path.
    path: PathBuf,
    /// Display name.
    name: String,
    /// How deep it sits, for indentation.
    depth: usize,
    /// Whether it is a directory.
    is_dir: bool,
    /// Whether it is ignored by git.
    ignored: bool,
    /// Whether a directory is open.
    expanded: bool,
    /// How many entries a directory holds, when it is known.
    children: Option<usize>,
}

/// Draws the tree.
///
/// @param state the application state
/// @param ui the interface to draw into
pub fn show(state: &mut HarnessState, ui: &mut Ui) {
    toolbar(state, ui);
    ui.add_space(4.0);
    filter(state, ui);
    ui.add_space(4.0);

    let root = state.root.clone();
    let mut rows = Vec::new();
    let filter_text = state.tree.filter.trim().to_lowercase();
    collect(&mut state.tree, &root, &root, 0, &filter_text, &mut rows);

    let selected = state
        .editor
        .buffer
        .as_ref()
        .map(|buffer| buffer.path.clone());
    let mut open: Option<PathBuf> = None;
    let mut toggle: Option<PathBuf> = None;

    ScrollArea::vertical()
        .id_salt("tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new("nothing here").size(11.5).color(theme::FAINT));
            }
            for row in &rows {
                let indent = row.depth as f32 * 11.0;
                let is_selected = selected.as_deref() == Some(row.path.as_path());
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let caret = if row.is_dir {
                        if row.expanded {
                            "▾"
                        } else {
                            "▸"
                        }
                    } else {
                        " "
                    };
                    ui.label(RichText::new(caret).size(10.0).color(theme::FAINT));
                    let colour = if is_selected {
                        theme::TEXT
                    } else if row.ignored {
                        theme::FAINT
                    } else if row.is_dir {
                        theme::BLUE
                    } else {
                        theme::DIM
                    };
                    let mut text = RichText::new(&row.name).size(12.0).color(colour);
                    if is_selected {
                        text = text.strong();
                    }
                    let response = ui.add(
                        egui::Label::new(text)
                            .sense(egui::Sense::click())
                            .truncate(),
                    );
                    if let Some(count) = row.children {
                        if !row.expanded {
                            ui.label(
                                RichText::new(format!("{count}"))
                                    .size(10.0)
                                    .color(theme::FAINT),
                            );
                        }
                    }
                    let response = response.on_hover_text(row.path.display().to_string());
                    if response.clicked() {
                        if row.is_dir {
                            toggle = Some(row.path.clone());
                        } else {
                            open = Some(row.path.clone());
                        }
                    }
                });
            }
        });

    if let Some(dir) = toggle {
        if state.tree.expanded.contains(&dir) {
            state.tree.expanded.remove(&dir);
        } else {
            state.tree.expanded.insert(dir);
        }
    }
    if let Some(path) = open {
        state.open_file(&path);
    }
}

/// The tree's header: what it is, and what it is hiding.
///
/// @param state the application state
/// @param ui the interface to draw into
fn toolbar(state: &mut HarnessState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        theme::section(ui, "files");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ignored = state.tree.show_ignored;
            let label = if ignored {
                "ignored: on"
            } else {
                "ignored: off"
            };
            if theme::action(
                ui,
                label,
                true,
                if ignored { theme::AMBER } else { theme::FAINT },
            )
            .on_hover_text("Show paths git ignores, dimmed")
            .clicked()
            {
                state.tree.show_ignored = !ignored;
                state.tree.invalidate();
            }
            if theme::action(ui, "refresh", true, theme::DIM).clicked() {
                state.tree.invalidate();
                state.refresh_palette_files();
            }
        });
    });
}

/// The tree's filter field.
///
/// @param state the application state
/// @param ui the interface to draw into
fn filter(state: &mut HarnessState, ui: &mut Ui) {
    ui.add(
        TextEdit::singleline(&mut state.tree.filter)
            .id(egui::Id::new("tree-filter"))
            .hint_text("filter names")
            .desired_width(ui.available_width())
            .font(theme::text(-1.0)),
    );
}

/// Flattens the visible part of the tree into rows.
///
/// @param tree the tree's cache and expansion state
/// @param root the workspace root, which the listings are relative to
/// @param dir the directory to read
/// @param depth how deep this call is
/// @param filter a lower-case filter, empty for no filtering
/// @param rows the rows being collected
fn collect(
    tree: &mut TreeState,
    root: &Path,
    dir: &Path,
    depth: usize,
    filter: &str,
    rows: &mut Vec<Row>,
) {
    if rows.len() >= TREE_ROWS {
        return;
    }
    let Some(listing) = tree.ensure(root, dir) else {
        return;
    };
    let entries: Vec<(String, PathBuf, bool, bool)> = listing
        .entries
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                entry.path.clone(),
                entry.is_dir,
                entry.ignored,
            )
        })
        .collect();
    let children = entries.len();

    for (name, path, is_dir, ignored) in entries {
        if rows.len() >= TREE_ROWS {
            return;
        }
        // The filter hides files that do not match; directories stay visible so
        // a match deep in the tree is still reachable by expanding.
        if !filter.is_empty() && !is_dir && !name.to_lowercase().contains(filter) {
            continue;
        }
        let expanded = is_dir && tree.expanded.contains(&path);
        rows.push(Row {
            path: path.clone(),
            name,
            depth,
            is_dir,
            ignored,
            expanded,
            children: if is_dir { Some(children) } else { None },
        });
        if expanded {
            collect(tree, root, &path, depth + 1, filter, rows);
        }
    }
}
