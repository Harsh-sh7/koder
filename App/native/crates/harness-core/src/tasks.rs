//! Task history: every conversation a workspace has had, kept on disk.
//!
//! A task is one conversation — its transcript, the engine session it ran in,
//! and the name the sidebar shows. Each lives in its own file under
//! `<workspace>/.harness/tasks/`, so a crash loses at most the task in flight,
//! two tasks never contend for one file, and the history survives the app, a
//! workspace switch, and a new task. The engine keeps its own session log; the
//! id recorded here is what lets a reopened task resume that session, so a
//! follow-up continues with the model's full context instead of starting cold.
//!
//! Writes go to a temporary file first and are renamed into place, so a task
//! file is always either the previous version or the new one, never half of
//! each.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::Item;

/// Most characters of the first prompt a task's name keeps.
const TITLE_CHARS: usize = 80;

/// One saved task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Stable id: the file name, and the key the sidebar selects by.
    pub id: String,
    /// The name the sidebar shows.
    pub title: String,
    /// Whether the user named it, so a later prompt does not rename it.
    #[serde(default)]
    pub renamed: bool,
    /// When the task began, as RFC 3339.
    pub created: String,
    /// When the task last changed, as RFC 3339; the history is newest first.
    pub updated: String,
    /// The engine session the transcript belongs to, when one was opened.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The route the task ran on, `provider/model`, when one was known.
    #[serde(default)]
    pub route: Option<String>,
    /// The transcript.
    #[serde(default)]
    pub items: Vec<Item>,
}

/// One row of the history, without its transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    /// The task's id.
    pub id: String,
    /// Its name.
    pub title: String,
    /// When it last changed, as RFC 3339.
    pub updated: String,
}

/// The task files of one workspace.
#[derive(Debug, Clone)]
pub struct TaskStore {
    dir: PathBuf,
}

impl TaskStore {
    /// The store for a workspace.
    ///
    /// @param workspace workspace root
    /// @returns the store; nothing is created until the first save
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            dir: workspace.join(".harness").join("tasks"),
        }
    }

    /// Where one task lives.
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// A fresh task id: sortable by time, unique within the process.
    ///
    /// @returns the id
    pub fn new_id() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let now = chrono::Local::now();
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed) % 1000;
        format!("{}-{serial:03}", now.format("%Y%m%d-%H%M%S"))
    }

    /// The name a task gets from what was first asked in it.
    ///
    /// @param prompt the first user message
    /// @returns one line, at most [`TITLE_CHARS`] characters
    pub fn title_for(prompt: &str) -> String {
        let line = prompt
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("New task");
        let line = line.trim_start_matches(['#', '>', '-', '*', ' ']);
        if line.chars().count() <= TITLE_CHARS {
            return line.to_string();
        }
        let mut cut: String = line.chars().take(TITLE_CHARS - 1).collect();
        if let Some(space) = cut.rfind(' ').filter(|at| *at > TITLE_CHARS / 2) {
            cut.truncate(space);
        }
        format!("{cut}…")
    }

    /// Every saved task, newest first.
    ///
    /// Unreadable files are skipped rather than failing the list: one damaged
    /// task must not hide the rest of the history.
    ///
    /// @returns the summaries
    pub fn list(&self) -> Vec<TaskSummary> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut tasks: Vec<TaskSummary> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| self.read(&entry.path()).ok())
            .map(|record| TaskSummary {
                id: record.id,
                title: record.title,
                updated: record.updated,
            })
            .collect();
        tasks.sort_by(|left, right| right.updated.cmp(&left.updated).then(right.id.cmp(&left.id)));
        tasks
    }

    /// Reads one task file.
    fn read(&self, path: &Path) -> Result<TaskRecord, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        serde_json::from_str(&raw).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Loads one task.
    ///
    /// @param id the task's id
    /// @returns the record, or why it could not be read
    pub fn load(&self, id: &str) -> Result<TaskRecord, String> {
        self.read(&self.path(id))
    }

    /// Writes one task, atomically.
    ///
    /// @param record the task
    /// @returns I/O or serialization failure
    pub fn save(&self, record: &TaskRecord) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|err| format!("{}: {err}", self.dir.display()))?;
        let body = serde_json::to_string(record).map_err(|err| err.to_string())?;
        let path = self.path(&record.id);
        let temporary = self.dir.join(format!(".{}.tmp", record.id));
        std::fs::write(&temporary, body).map_err(|err| format!("{}: {err}", temporary.display()))?;
        std::fs::rename(&temporary, &path).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Renames one task; the name then sticks through later prompts.
    ///
    /// @param id the task's id
    /// @param title the new name
    /// @returns why it could not be renamed
    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let mut record = self.load(id)?;
        let title = title.trim();
        if title.is_empty() {
            return Err("a task needs a name".to_string());
        }
        record.title = title.to_string();
        record.renamed = true;
        self.save(&record)
    }

    /// Deletes one task's file.
    ///
    /// @param id the task's id
    /// @returns why it could not be deleted
    pub fn delete(&self, id: &str) -> Result<(), String> {
        std::fs::remove_file(self.path(id)).map_err(|err| format!("{id}: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::NoticeLevel;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-tasks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn record(id: &str, updated: &str, prompt: &str) -> TaskRecord {
        TaskRecord {
            id: id.to_string(),
            title: TaskStore::title_for(prompt),
            renamed: false,
            created: updated.to_string(),
            updated: updated.to_string(),
            session_id: Some(format!("session-{id}")),
            route: Some("default/deepseek".to_string()),
            items: vec![
                Item::User { id: "u1".into(), text: prompt.into() },
                Item::Notice { level: NoticeLevel::Info, text: "done".into() },
            ],
        }
    }

    #[test]
    fn a_saved_task_reads_back_whole_and_the_history_is_newest_first() {
        let store = TaskStore::for_workspace(&scratch("roundtrip"));
        assert!(store.list().is_empty(), "no directory is an empty history, not an error");
        store.save(&record("a", "2026-09-27T10:00:00+05:30", "Build a todo app")).unwrap();
        store.save(&record("b", "2026-09-27T11:00:00+05:30", "Fix the parser")).unwrap();
        let listed: Vec<String> = store.list().into_iter().map(|task| task.id).collect();
        assert_eq!(listed, ["b", "a"]);
        let loaded = store.load("a").unwrap();
        assert_eq!(loaded.title, "Build a todo app");
        assert_eq!(loaded.session_id.as_deref(), Some("session-a"));
        assert_eq!(loaded.items.len(), 2);
    }

    #[test]
    fn rename_sticks_delete_removes_and_a_damaged_file_hides_nothing_else() {
        let root = scratch("edit");
        let store = TaskStore::for_workspace(&root);
        store.save(&record("a", "2026-09-27T10:00:00+05:30", "one")).unwrap();
        store.save(&record("b", "2026-09-27T11:00:00+05:30", "two")).unwrap();
        store.rename("a", "  Todo app  ").unwrap();
        let renamed = store.load("a").unwrap();
        assert_eq!(renamed.title, "Todo app");
        assert!(renamed.renamed);
        assert!(store.rename("a", "   ").is_err());
        std::fs::write(root.join(".harness/tasks/broken.json"), "{ not json").unwrap();
        assert_eq!(store.list().len(), 2);
        store.delete("b").unwrap();
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn a_title_is_the_first_real_line_cut_at_a_word() {
        assert_eq!(TaskStore::title_for("\n\n# Build a CLI\nmore"), "Build a CLI");
        assert_eq!(TaskStore::title_for("   "), "New task");
        let long = "word ".repeat(40);
        let title = TaskStore::title_for(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= TITLE_CHARS);
        assert!(!title.contains("wor…"), "cut at a word boundary: {title}");
        let first = TaskStore::new_id();
        assert_ne!(first, TaskStore::new_id(), "ids are unique within the process");
    }
}
