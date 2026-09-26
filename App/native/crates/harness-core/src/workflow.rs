//! Saved workflows — this application's equivalent of a snippet library.
//!
//! A workflow is a command or a prompt with a name, a description, and optional
//! `{{placeholders}}` the user fills in before it runs. They live as JSON files
//! under `<workspace>/.harness/workflows/`, one per file, so they travel with
//! the workspace and can be edited, diffed, and reviewed like anything else in
//! it — no hidden database keyed to this machine.
//!
//! The seeded set is deliberately small and short. Every seeded prompt is a
//! prompt that asks for less output rather than more, because a workflow that
//! makes the model write an essay is a workflow that costs tokens every time it
//! is used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whether a workflow types a command into the terminal or sends a prompt to
/// the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowKind {
    /// Runs in the terminal pane.
    Command,
    /// Sent to the agent as a prompt.
    Prompt,
}

impl WorkflowKind {
    /// A label for the pane.
    ///
    /// @returns the kind in words
    pub fn label(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Prompt => "prompt",
        }
    }
}

/// One `{{name}}` a workflow asks the user to fill in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowVariable {
    /// Placeholder name, without the braces.
    pub name: String,
    /// What to put there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Used when the user leaves it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Offered choices, when the value is one of a few.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl WorkflowVariable {
    /// A variable with only a name.
    ///
    /// @param name placeholder name
    /// @returns the variable
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            default: None,
            options: Vec::new(),
        }
    }

    /// The same variable with a description.
    ///
    /// @param description what to put in the placeholder
    /// @returns the variable
    pub fn described(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// The same variable with a default value.
    ///
    /// @param value used when the user leaves it empty
    /// @returns the variable
    pub fn defaulting_to(mut self, value: &str) -> Self {
        self.default = Some(value.to_string());
        self
    }
}

/// A saved command or prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Stable identifier; also the file name under the workflows directory.
    pub id: String,
    /// What the list shows.
    pub name: String,
    /// One line explaining when to reach for it.
    #[serde(default)]
    pub description: String,
    /// Command or prompt.
    pub kind: WorkflowKind,
    /// The command line or prompt text, with `{{placeholders}}`.
    pub body: String,
    /// The placeholders the body expects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<WorkflowVariable>,
    /// Free-form tags for grouping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Whether this shipped with the harness rather than being authored here.
    #[serde(default)]
    pub seeded: bool,
}

impl Workflow {
    /// A command workflow.
    ///
    /// @param id stable identifier
    /// @param name display name
    /// @param body command line
    /// @returns the workflow
    pub fn command(id: &str, name: &str, body: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            kind: WorkflowKind::Command,
            body: body.to_string(),
            variables: Vec::new(),
            tags: Vec::new(),
            seeded: false,
        }
    }

    /// A prompt workflow.
    ///
    /// @param id stable identifier
    /// @param name display name
    /// @param body prompt text
    /// @returns the workflow
    pub fn prompt(id: &str, name: &str, body: &str) -> Self {
        Self {
            kind: WorkflowKind::Prompt,
            ..Self::command(id, name, body)
        }
    }

    /// The same workflow with a description.
    ///
    /// @param description one line, for the list
    /// @returns the workflow
    pub fn describing(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }

    /// The same workflow with tags.
    ///
    /// @param tags free-form labels
    /// @returns the workflow
    pub fn tagged(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|tag| tag.to_string()).collect();
        self
    }

    /// The same workflow with variables.
    ///
    /// @param variables placeholders the body expects
    /// @returns the workflow
    pub fn with(mut self, variables: Vec<WorkflowVariable>) -> Self {
        self.variables = variables;
        self
    }

    /// The same workflow marked as seeded.
    ///
    /// @returns the workflow
    pub fn seeded(mut self) -> Self {
        self.seeded = true;
        self
    }

    /// Every `{{name}}` in the body, in order of first appearance.
    ///
    /// @returns placeholder names without braces
    pub fn placeholders(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut rest = self.body.as_str();
        while let Some(open) = rest.find("{{") {
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else { break };
            let name = after[..close].trim();
            if !name.is_empty() && !names.iter().any(|seen| seen == name) {
                names.push(name.to_string());
            }
            rest = &after[close + 2..];
        }
        names
    }

    /// Fills the body's placeholders.
    ///
    /// A value wins over a declared default; a placeholder with neither is left
    /// standing and reported, so the user sees exactly what is missing instead
    /// of sending the model a sentence with a hole in it.
    ///
    /// @param values placeholder values, by name
    /// @returns the filled body and the names that had nothing to fill them
    pub fn render(&self, values: &BTreeMap<String, String>) -> Rendered {
        let mut missing = Vec::new();
        let mut text = String::with_capacity(self.body.len());
        let mut rest = self.body.as_str();
        while let Some(open) = rest.find("{{") {
            text.push_str(&rest[..open]);
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else {
                text.push_str(&rest[open..]);
                return Rendered { text, missing };
            };
            let name = after[..close].trim();
            let value = values
                .get(name)
                .filter(|value| !value.is_empty())
                .map(String::as_str)
                .or_else(|| self.default_of(name));
            match value {
                Some(value) => text.push_str(value),
                None => {
                    if !missing.iter().any(|seen| seen == name) {
                        missing.push(name.to_string());
                    }
                    text.push_str(&rest[open..open + 2 + close + 2]);
                }
            }
            rest = &after[close + 2..];
        }
        text.push_str(rest);
        Rendered { text, missing }
    }

    /// The declared default for a placeholder, if any.
    fn default_of(&self, name: &str) -> Option<&str> {
        self.variables
            .iter()
            .find(|variable| variable.name == name)
            .and_then(|variable| variable.default.as_deref())
    }
}

/// A rendered workflow body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The body with every value that was available filled in.
    pub text: String,
    /// Placeholders that had neither a value nor a default.
    pub missing: Vec<String>,
}

/// The workspace's saved workflows.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    dir: PathBuf,
    workflows: Vec<Workflow>,
}

impl WorkflowStore {
    /// Loads the store, seeding a fresh directory with the built-in set.
    ///
    /// A workflow file that does not parse is skipped with a log line rather
    /// than failing the load: one bad file should not take the pane down.
    ///
    /// @param dir the workflows directory
    /// @returns the store
    pub fn load(dir: &Path) -> Self {
        let mut workflows = Vec::new();
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(raw) => match serde_json::from_str::<Workflow>(&raw) {
                            Ok(workflow) => workflows.push(workflow),
                            Err(err) => log::warn!(
                                "workflow: {} is unreadable ({err}); skipped",
                                path.display()
                            ),
                        },
                        Err(err) => log::warn!(
                            "workflow: {} is unreadable ({err}); skipped",
                            path.display()
                        ),
                    }
                }
            }
            Err(_) => {
                let seeded = seed();
                let store = Self {
                    dir: dir.to_path_buf(),
                    workflows: seeded,
                };
                if let Err(err) = store.persist_all() {
                    log::warn!("workflow: could not seed {}: {err}", dir.display());
                }
                return store;
            }
        }
        workflows.sort_by(|a, b| {
            a.kind
                .label()
                .cmp(b.kind.label())
                .then_with(|| a.name.cmp(&b.name))
        });
        Self {
            dir: dir.to_path_buf(),
            workflows,
        }
    }

    /// @returns the directory the workflows live in
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// @returns every workflow, commands and prompts together
    pub fn all(&self) -> &[Workflow] {
        &self.workflows
    }

    /// @param id workflow identifier
    /// @returns the workflow, when there is one
    pub fn get(&self, id: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|workflow| workflow.id == id)
    }

    /// @param kind command or prompt
    /// @returns the workflows of that kind
    pub fn of_kind(&self, kind: WorkflowKind) -> Vec<&Workflow> {
        self.workflows
            .iter()
            .filter(|workflow| workflow.kind == kind)
            .collect()
    }

    /// Adds or replaces a workflow and writes it to disk.
    ///
    /// @param workflow the workflow to store
    /// @throws `Err` when the file cannot be written
    pub fn upsert(&mut self, workflow: Workflow) -> Result<(), String> {
        match self
            .workflows
            .iter_mut()
            .find(|existing| existing.id == workflow.id)
        {
            Some(existing) => *existing = workflow.clone(),
            None => self.workflows.push(workflow.clone()),
        }
        self.workflows.sort_by(|a, b| {
            a.kind
                .label()
                .cmp(b.kind.label())
                .then_with(|| a.name.cmp(&b.name))
        });
        self.write(&workflow)
    }

    /// Removes a workflow and deletes its file.
    ///
    /// @param id workflow identifier
    /// @returns whether anything was removed
    /// @throws `Err` when the file exists and cannot be deleted
    pub fn remove(&mut self, id: &str) -> Result<bool, String> {
        let before = self.workflows.len();
        self.workflows.retain(|workflow| workflow.id != id);
        if self.workflows.len() == before {
            return Ok(false);
        }
        let path = self.path_for(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(err) => Err(format!("{}: {err}", path.display())),
        }
    }

    /// Writes one workflow's file.
    ///
    /// @param workflow the workflow to write
    /// @throws `Err` when the file cannot be written
    pub fn write(&self, workflow: &Workflow) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| format!("{}: {err}", self.dir.display()))?;
        let path = self.path_for(&workflow.id);
        let body = serde_json::to_string_pretty(workflow).map_err(|err| err.to_string())?;
        std::fs::write(&path, format!("{body}\n"))
            .map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Writes every workflow, used when seeding.
    fn persist_all(&self) -> Result<(), String> {
        for workflow in &self.workflows {
            self.write(workflow)?;
        }
        Ok(())
    }

    /// @param id workflow identifier
    /// @returns the file a workflow is stored in
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", slug(id)))
    }
}

/// The built-in workflows a fresh workspace starts with.
///
/// @returns the seeded set
pub fn seed() -> Vec<Workflow> {
    vec![
        Workflow::prompt(
            "explain-failure",
            "Explain the failure",
            "The last command in this workspace failed. Explain the cause in two sentences, then give the smallest fix \
             as a single diff. Skip the background.",
        )
        .describing("Diagnose the last terminal failure, briefly")
        .tagged(&["debug"])
        .seeded(),
        Workflow::prompt(
            "review-changes",
            "Review my changes",
            "Review the uncommitted changes in this workspace. List only real problems: correctness bugs, missed edge \
             cases, and missing tests. One line each, no praise, no style notes.",
        )
        .describing("Find correctness bugs in the working tree")
        .tagged(&["review"])
        .seeded(),
        Workflow::prompt(
            "commit-message",
            "Write a commit message",
            "Write one conventional-commit subject line for the staged changes. Reply with the line only.",
        )
        .describing("One-line commit subject for what is staged")
        .tagged(&["git"])
        .seeded(),
        Workflow::prompt(
            "summarise-file",
            "Summarise a file",
            "Summarise {{path}} in five bullets: what it does, what it depends on, who calls it, what it risks, and \
             what a newcomer must know. No code quotes.",
        )
        .describing("Orient yourself in an unfamiliar file")
        .tagged(&["explore"])
        .with(vec![WorkflowVariable::new("path").described("File to summarise")])
        .seeded(),
        Workflow::command("run-checks", "Run the checks", "make check")
            .describing("Typecheck and test everything a reviewer can run")
            .tagged(&["build"])
            .seeded(),
    ]
}

/// Turns an identifier into a file name that no filesystem will argue with.
///
/// @param id workflow identifier
/// @returns a lowercase name of letters, digits, dashes, and underscores
pub fn slug(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '/' || ch == '.' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workflows directory that removes itself.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("harness-workflow-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            Self { dir }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_fresh_directory_is_seeded_and_survives_a_reload() {
        let fixture = Fixture::new("seed");
        let store = WorkflowStore::load(&fixture.dir);
        assert_eq!(store.all().len(), seed().len());
        assert!(store.get("explain-failure").is_some());
        assert!(
            fixture.dir.join("run-checks.json").is_file(),
            "seeds are written, not just held"
        );

        let reloaded = WorkflowStore::load(&fixture.dir);
        assert_eq!(reloaded.all().len(), store.all().len());
        assert_eq!(
            reloaded.get("review-changes").unwrap().kind,
            WorkflowKind::Prompt
        );
    }

    #[test]
    fn placeholders_are_listed_and_filled_from_values_defaults_or_neither() {
        let workflow = Workflow::prompt(
            "summarise",
            "Summarise",
            "Summarise {{path}} for {{audience}}.",
        )
        .with(vec![
            WorkflowVariable::new("path").described("file"),
            WorkflowVariable::new("audience").defaulting_to("a new contributor"),
        ]);
        assert_eq!(workflow.placeholders(), vec!["path", "audience"]);

        let filled = workflow.render(&values(&[("path", "src/lib.rs")]));
        assert_eq!(filled.text, "Summarise src/lib.rs for a new contributor.");
        assert!(filled.missing.is_empty());

        let partial = workflow.render(&BTreeMap::new());
        assert_eq!(partial.text, "Summarise {{path}} for a new contributor.");
        assert_eq!(partial.missing, vec!["path"]);

        let explicit = workflow.render(&values(&[("path", "x"), ("audience", "a reviewer")]));
        assert_eq!(explicit.text, "Summarise x for a reviewer.");

        let empty_value_falls_back = workflow.render(&values(&[("path", ""), ("audience", "")]));
        assert_eq!(
            empty_value_falls_back.text,
            "Summarise {{path}} for a new contributor."
        );
    }

    #[test]
    fn a_body_without_placeholders_renders_unchanged() {
        let workflow = Workflow::command("status", "Status", "git status --short");
        assert!(workflow.placeholders().is_empty());
        let rendered = workflow.render(&values(&[("unused", "x")]));
        assert_eq!(rendered.text, "git status --short");
        assert!(rendered.missing.is_empty());
    }

    #[test]
    fn upsert_replaces_by_id_and_remove_deletes_the_file() {
        let fixture = Fixture::new("upsert");
        let mut store = WorkflowStore::load(&fixture.dir);
        let count = store.all().len();

        store
            .upsert(Workflow::prompt(
                "explain-failure",
                "Explain it again",
                "Explain the last failure in one line.",
            ))
            .expect("upsert");
        assert_eq!(
            store.all().len(),
            count,
            "the id is what identifies a workflow"
        );
        assert_eq!(
            store.get("explain-failure").unwrap().name,
            "Explain it again"
        );
        assert!(!store.get("explain-failure").unwrap().seeded);

        store
            .upsert(Workflow::command("deploy", "Deploy", "make run"))
            .expect("upsert");
        assert_eq!(store.all().len(), count + 1);
        assert!(fixture.dir.join("deploy.json").is_file());

        assert!(store.remove("deploy").expect("remove"));
        assert!(!store.remove("deploy").expect("remove again"));
        assert!(!fixture.dir.join("deploy.json").exists());
        assert_eq!(WorkflowStore::load(&fixture.dir).all().len(), count);
    }

    #[test]
    fn a_workflow_file_that_does_not_parse_is_skipped_without_losing_the_others() {
        let fixture = Fixture::new("broken");
        WorkflowStore::load(&fixture.dir);
        std::fs::write(fixture.dir.join("broken.json"), "{ not json").unwrap();
        let store = WorkflowStore::load(&fixture.dir);
        assert!(store.get("explain-failure").is_some());
        assert_eq!(store.all().len(), seed().len());
    }

    #[test]
    fn kinds_are_separated_for_the_pane() {
        let fixture = Fixture::new("kinds");
        let store = WorkflowStore::load(&fixture.dir);
        assert!(store.of_kind(WorkflowKind::Prompt).len() >= 4);
        assert_eq!(store.of_kind(WorkflowKind::Command).len(), 1);
        assert_eq!(
            store.of_kind(WorkflowKind::Command)[0].kind.label(),
            "command"
        );
    }

    #[test]
    fn identifiers_become_file_names() {
        assert_eq!(slug("Run checks"), "run-checks");
        assert_eq!(slug("src/main.rs"), "src-main-rs");
        assert_eq!(slug("already-fine"), "already-fine");
        assert_eq!(slug("!!!"), "workflow");
    }
}
