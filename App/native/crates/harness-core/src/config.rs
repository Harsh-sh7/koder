//! Harness configuration: where the workspace state lives, and the model routes
//! the agent engine reads.
//!
//! Two documents, deliberately separate:
//!
//! - `.harness/models.json` is the **route registry**. The dsh composition mounts
//!   our provider plugin with `modelsFile: .harness/models.json`, so the file is
//!   the only place a route can appear — this app writes it, the engine reads it,
//!   and no source change is needed to add a model. The schema is the provider's
//!   (`providers` keyed by route name, plus `active` recording the app's own
//!   choice), which is why the writer here is strict about the fields it emits:
//!   the provider refuses a document with an unknown field.
//! - `.harness/config.json` is this **application's own** document: UI
//!   preferences, the soft caps mirrored for display, and the last workspace
//!   opened. The engine never reads it.
//!
//! The engine itself is launched as a child process; [`EnginePaths`] resolves the
//! checkout, the Node interpreter, and the launcher script, so the app can find
//! its own engine without the user typing a path.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One model entry inside a provider profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderModel {
    /// Provider-facing model id, e.g. `deepseek-chat`.
    pub id: String,
    /// Display name shown in the model picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Context window in tokens; drives the context-usage meter.
    #[serde(
        rename = "contextWindow",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<u32>,
    /// Maximum output tokens requested per call.
    #[serde(rename = "maxTokens", default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// One provider route: everything the provider plugin needs to serve it.
///
/// Only the fields this application writes are modelled; a document written by
/// hand may carry more of the provider's vocabulary, and reading keeps them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// Human label shown in the model picker.
    #[serde(
        rename = "displayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<String>,
    /// API family; `openai-completions` is the OpenAI-compatible default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// API root, e.g. `https://api.deepseek.com/v1`.
    #[serde(rename = "baseURL", default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Environment variable holding the credential. Defaults to `AI_API_KEY`
    /// inside the provider; naming it here is what keeps the key out of a file.
    #[serde(rename = "apiKeyEnv", default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Models this route serves. Omitted means "whatever the installed catalog
    /// ships under this key", which is how a documented provider is one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ProviderModel>>,
}

/// The route a session starts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveRoute {
    /// Route name, as keyed in [`ModelsDocument::providers`].
    pub provider: String,
    /// Model id within that route.
    pub model: String,
}

/// The workspace route registry, exactly as `llm-harness-provider` parses it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelsDocument {
    /// Route profiles keyed by route name.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderProfile>,
    /// The app's selected route; absent until the user picks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveRoute>,
}

impl ModelsDocument {
    /// Reads the document; a missing file is an empty registry.
    ///
    /// A file that exists but does not parse is reported, not ignored: silently
    /// falling back would run the session on a different route than the
    /// document says.
    ///
    /// @param path document path
    /// @returns parsed document, or an empty registry when absent
    /// @throws `Err` with the parse failure when the file exists and is invalid
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        serde_json::from_str(&raw).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Writes the document, creating `.harness/` as needed.
    ///
    /// @returns I/O or serialization error
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, format!("{body}\n"))
    }

    /// Every (route, model) pair the document offers, in display order.
    ///
    /// A route with no `models` list contributes nothing: this application
    /// writes explicit model lists, and a certificate-less catalog route cannot
    /// be offered in a picker without asking the provider what it serves.
    ///
    /// @returns picker entries of `(route, model id, display label)`
    pub fn routes(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (route, profile) in &self.providers {
            let route_label = profile
                .display_name
                .clone()
                .unwrap_or_else(|| route.clone());
            for model in profile.models.as_deref().unwrap_or_default() {
                let label = model.name.clone().unwrap_or_else(|| model.id.clone());
                out.push((
                    route.clone(),
                    model.id.clone(),
                    format!("{route_label} · {label}"),
                ));
            }
        }
        out
    }

    /// Adds or replaces one route's model list, leaving other routes untouched.
    ///
    /// @param route route key
    /// @param profile the profile to store under that key
    pub fn upsert_route(&mut self, route: &str, profile: ProviderProfile) {
        self.providers.insert(route.to_string(), profile);
    }
}

/// USD price per million tokens.
///
/// Prices are the application's own data, not the engine's: the provider
/// document refuses fields it does not know, so a price table lives beside it
/// and is optional. A route without prices shows tokens and no cost claim.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    /// Prompt tokens.
    pub input: f64,
    /// Completion tokens.
    pub output: f64,
    /// Cached prompt-read tokens, when the provider prices them separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
}

/// Optional price table, keyed `route/model`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    /// Prices by `route/model` key.
    #[serde(default)]
    pub models: BTreeMap<String, Price>,
}

impl Prices {
    /// Reads the table; a missing file means no prices are known.
    ///
    /// @param path document path
    /// @returns parsed table, or an empty one when absent
    /// @throws `Err` with the parse failure when the file exists and is invalid
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        serde_json::from_str(&raw).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Writes the table, creating its directory when it is missing.
    ///
    /// @param path document path
    /// @returns the write result
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, format!("{body}\n"))
    }

    /// The price for one route and model, when either spelling is present.
    ///
    /// @param route route name
    /// @param model model id
    /// @returns the price, or `None` when the table has nothing for this pair
    pub fn get(&self, route: &str, model: &str) -> Option<Price> {
        self.models
            .get(&format!("{route}/{model}"))
            .or_else(|| self.models.get(model))
            .copied()
    }
}

/// Hard caps the harness enforces engine-side, mirrored here so the UI can show
/// the ceiling it will be held to rather than guessing at one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapsConfig {
    /// Model tokens one session may spend before the harness stops it.
    pub token_budget_per_session: u64,
    /// Fraction of the ceiling at which the engine warns the model once.
    pub warn_at: f32,
    /// Model steps one turn may take.
    pub steps_per_turn: u32,
    /// Approved escalations past the ceiling, and the tokens each buys.
    pub max_escalations: u32,
    /// Tokens one approved escalation adds.
    pub grant_tokens: u64,
}

impl Default for CapsConfig {
    fn default() -> Self {
        // These mirror harness-preset's defaults so the meter and the engine
        // agree out of the box; the engine's values win if a profile changes them.
        Self {
            token_budget_per_session: 400_000,
            warn_at: 0.8,
            steps_per_turn: 60,
            max_escalations: 2,
            grant_tokens: 200_000,
        }
    }
}

/// This application's own settings document.
///
/// There is deliberately no theme setting: the harness has one visual system,
/// and a knob that does nothing is a knob that lies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Interface font size in points.
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Terminal font size in points.
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    /// Whether the session panel starts open.
    #[serde(default = "default_true")]
    pub agent_panel_open: bool,
    /// Whether the file-tree column starts open.
    #[serde(default)]
    pub tree_open: bool,
    /// Whether the sidebar starts expanded.
    #[serde(default = "default_true")]
    pub sidebar_open: bool,
    /// Caps mirrored for display.
    #[serde(default)]
    pub caps: CapsConfig,
}

fn default_font_size() -> f32 {
    13.5
}
fn default_terminal_font_size() -> f32 {
    13.0
}
fn default_true() -> bool {
    true
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            terminal_font_size: default_terminal_font_size(),
            agent_panel_open: true,
            tree_open: false,
            sidebar_open: true,
            caps: CapsConfig::default(),
        }
    }
}

impl HarnessConfig {
    /// Loads the app document from `<workspace>/.harness/config.json`.
    ///
    /// @param workspace workspace root
    /// @returns parsed settings, or the defaults when absent or unreadable
    pub fn load(workspace: &Path) -> Self {
        let path = Self::path_for(workspace);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
                log::warn!(
                    "config: {} is unreadable ({err}); using defaults",
                    path.display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persists the app document, creating `.harness/` as needed.
    ///
    /// @returns I/O or serialization error
    pub fn save(&self, workspace: &Path) -> std::io::Result<()> {
        let path = Self::path_for(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, format!("{body}\n"))
    }

    /// @param workspace workspace root
    /// @returns path to `<workspace>/.harness/config.json`
    pub fn path_for(workspace: &Path) -> PathBuf {
        workspace.join(".harness").join("config.json")
    }

    /// @param workspace workspace root
    /// @returns path to `<workspace>/.harness/models.json`
    pub fn models_path_for(workspace: &Path) -> PathBuf {
        workspace.join(".harness").join("models.json")
    }

    /// @param workspace workspace root
    /// @returns path to `<workspace>/.harness/prices.json`
    pub fn prices_path_for(workspace: &Path) -> PathBuf {
        workspace.join(".harness").join("prices.json")
    }

    /// @param workspace workspace root
    /// @returns directory holding this workspace's saved workflows
    pub fn workflows_dir_for(workspace: &Path) -> PathBuf {
        workspace.join(".harness").join("workflows")
    }
}

/// Where the harness engine lives on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct EnginePaths {
    /// Repository root (the directory holding `Makefile` and `App/`).
    pub repo_root: PathBuf,
    /// The vendored dsh checkout.
    pub dsh_repo: PathBuf,
    /// Node interpreter dsh supports.
    pub node: PathBuf,
    /// The launcher script that boots the profile as an ACP server.
    pub launcher: PathBuf,
    /// `$DSH_HOME` the profile is materialized under.
    pub dsh_home: PathBuf,
}

impl EnginePaths {
    /// Resolves the engine layout for a workspace.
    ///
    /// Discovery order is explicit > environment > the checkout the running
    /// binary belongs to > an ancestor of the workspace. The binary's own
    /// location is tried first because a shipped app must work from any current
    /// directory; ancestors of the workspace are the case of a developer running
    /// the debug binary from inside the repository.
    ///
    /// @param workspace workspace root the app was launched for
    /// @param dsh_home explicit Harness home, when the caller named one
    /// @returns resolved paths, or an error naming what could not be found
    pub fn resolve(workspace: &Path, dsh_home: Option<PathBuf>) -> Result<Self, String> {
        let repo_root = std::env::var_os("HARNESS_REPO")
            .map(PathBuf::from)
            .filter(|path| is_repo_root(path))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| ancestors(&exe).find(|dir| is_repo_root(dir)))
            })
            .or_else(|| ancestors(workspace).find(|dir| is_repo_root(dir)))
            .ok_or_else(|| {
                "no harness checkout found: run from inside the repository, or set HARNESS_REPO"
                    .to_string()
            })?;
        let dsh_repo = repo_root.join("Deepseek").join("deepseek-harness");
        let launcher = repo_root.join("App").join("scripts").join("run-acp.mjs");
        let node = find_node(&repo_root)?;
        let dsh_home = dsh_home.unwrap_or_else(|| repo_root.join(".dsh"));
        Ok(Self {
            repo_root,
            dsh_repo,
            node,
            launcher,
            dsh_home,
        })
    }

    /// The command that starts the engine as an ACP server on stdio.
    ///
    /// @returns program and arguments, ready for `Command::new`
    pub fn engine_command(&self) -> (PathBuf, Vec<String>) {
        (
            self.node.clone(),
            vec![self.launcher.to_string_lossy().into_owned()],
        )
    }
}

/// Whether a directory is the harness repository root.
fn is_repo_root(dir: &Path) -> bool {
    dir.join("App")
        .join("scripts")
        .join("run-acp.mjs")
        .is_file()
}

/// A directory and every ancestor, nearest first.
fn ancestors(start: &Path) -> impl Iterator<Item = PathBuf> {
    let start = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    start
        .ancestors()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>()
        .into_iter()
}

/// Finds a Node interpreter dsh supports (`^22.19` or `>=24`).
///
/// A homebrew `node@24` sitting beside an older default is the common case on a
/// developer machine, so the well-known prefixes are probed after `PATH` and the
/// version is always checked rather than assumed. `.toolchain/node` is the copy
/// `make setup` installs on a machine that has none (App/scripts/provision-node.sh),
/// so an application built there finds the same interpreter the Makefile uses.
///
/// @param repo_root repository root, whose `.toolchain/` may hold a pinned Node
/// @returns absolute interpreter path
/// @throws `Err` naming the requirement when no supported interpreter exists
pub fn find_node(repo_root: &Path) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("HARNESS_NODE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(
        repo_root
            .join(".toolchain")
            .join("node")
            .join("bin")
            .join("node"),
    );
    if let Some(path) = which_node() {
        candidates.push(path);
    }
    candidates.push(PathBuf::from("/opt/homebrew/opt/node@24/bin/node"));
    candidates.push(PathBuf::from("/usr/local/opt/node@24/bin/node"));
    for candidate in &candidates {
        if node_is_supported(candidate) {
            return Ok(candidate.clone());
        }
    }
    Err(format!(
        "no supported node found (dsh needs ^22.19 or >=24); tried {}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The first `node` on `PATH`, if any.
fn which_node() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("node"))
        .find(|candidate| candidate.is_file())
}

/// Whether an interpreter satisfies dsh's engine range.
fn node_is_supported(node: &Path) -> bool {
    let Ok(output) = std::process::Command::new(node).arg("--version").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let mut parts = raw.trim().trim_start_matches('v').split('.');
    let major: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    major > 22 || (major == 22 && minor >= 19)
}

/// Loads `KEY=value` pairs from `<root>/.env` into the process environment
/// without replacing variables already set.
///
/// Only `AI_*` names are honoured: the harness reads `AI_API_KEY`, and a stray
/// `DEEPSEEK_API_KEY` in a project file is never picked up for the app's own
/// model calls.
///
/// @param root directory that may contain a `.env` file
/// @returns names of the variables that were newly set
pub fn load_dotenv(root: &Path) -> Vec<String> {
    apply_dotenv(root, false)
}

/// Loads `<root>/.env` over the process environment, replacing what is there.
///
/// This is the workspace-switch path: the folder being opened owns its own
/// `.env`, and a key written for it must win over the one the app started with.
///
/// @param root directory that may contain a `.env` file
/// @returns names of the variables that were set
pub fn load_dotenv_over(root: &Path) -> Vec<String> {
    apply_dotenv(root, true)
}

/// Validate a provider endpoint before storing it or sending a request.
/// @param value the endpoint entered in the route editor
/// @returns the trimmed endpoint, or a repair message without echoing credentials
pub fn validate_base_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let invalid = "the base URL must be an absolute http:// or https:// URL with a host";
    if value.chars().any(char::is_whitespace) || !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(invalid.into());
    }
    let parsed = url::Url::parse(value).map_err(|_| invalid.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(invalid.into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("the base URL must not contain credentials, a query, or a fragment; enter the API key separately".into());
    }
    Ok(value.trim_end_matches('/').to_string())
}

/// Reads `<root>/.env` and applies its `AI_*` pairs.
///
/// @param root directory that may contain a `.env` file
/// @param override_existing whether a pair replaces an already-set variable
/// @returns names of the variables that were applied
fn apply_dotenv(root: &Path, override_existing: bool) -> Vec<String> {
    let mut applied = Vec::new();
    for (key, value) in dotenv_values(root) {
        if override_existing || std::env::var_os(&key).is_none() {
            std::env::set_var(&key, value);
            applied.push(key);
        }
    }
    applied
}

/// Read workspace settings without changing the application's global environment.
/// @param root directory holding the optional dotenv file
/// @returns AI settings to pass directly to a newly launched engine
pub fn dotenv_values(root: &Path) -> BTreeMap<String, String> {
    let raw = std::fs::read_to_string(root.join(".env")).unwrap_or_default();
    raw.lines().filter_map(|line| {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        if !key.starts_with("AI_") || !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return None;
        }
        let value = value.trim();
        let value = if value.len() >= 2 && ((value.starts_with('"') && value.ends_with('"')) || (value.starts_with('\'') && value.ends_with('\''))) {
            &value[1..value.len() - 1]
        } else {
            value.split(" #").next().unwrap_or(value).trim_end()
        };
        Some((key.to_string(), value.to_string()))
    }).collect()
}

/// Writes one `KEY=value` pair into `<root>/.env`, creating the file when it is
/// missing and leaving every other line as it was.
///
/// `.env` is where a credential's *value* belongs: the route document records
/// only the variable's name, so the model dialog's key field lands here, in a
/// file the repository ignores. Only `AI_*` names are accepted, mirroring
/// [`load_dotenv`] — a pair written under any other name would never be read
/// back, which is a failure this can refuse instead of leaving to a later
/// mystery.
///
/// @param root directory whose `.env` should carry the pair
/// @param key variable name, e.g. `AI_API_KEY`
/// @param value the credential
/// @returns the path written
/// @throws `Err` naming what refused the pair
pub fn write_dotenv_value(root: &Path, key: &str, value: &str) -> Result<PathBuf, String> {
    let key = key.trim();
    let value = value.trim();
    if !key.starts_with("AI_") {
        return Err(format!(
            "{key} is never read: this harness honours AI_* variables only"
        ));
    }
    if value.is_empty() {
        return Err("a credential cannot be empty".to_string());
    }
    if value.contains(['\n', '\r']) {
        return Err("a credential cannot span lines".to_string());
    }
    let path = root.join(".env");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut replaced = false;
    let lines: Vec<String> = existing
        .lines()
        .filter_map(|line| {
            let head = line.trim_start();
            let named = !head.starts_with('#')
                && head
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim() == key);
            if !named {
                return Some(line.to_string());
            }
            if replaced {
                // A duplicate assignment of the same name is dead weight: the
                // first line is the one the reader takes.
                return None;
            }
            replaced = true;
            Some(format!("{key}={value}"))
        })
        .collect();
    let mut body = lines.join("\n");
    if replaced {
        body.push('\n');
    } else {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!("{key}={value}\n"));
    }
    std::fs::write(&path, body).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn endpoint_validation_rejects_bad_urls_without_echoing_secrets() {
        for bad in ["", "openrouter.ai/api/v1", "https://", "file:///tmp/api", "https://host/v1 secret", "https://secret@host/v1", "https://host/v1?key=secret"] {
            let error = validate_base_url(bad).unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert_eq!(validate_base_url(" https://openrouter.ai/api/v1/ ").unwrap(), "https://openrouter.ai/api/v1");
        assert!(validate_base_url("http://127.0.0.1:8899/v1").is_ok());
    }

    #[test]
    fn missing_models_document_is_an_empty_registry() {
        let dir = temp_dir("models-absent");
        let doc = ModelsDocument::load(&HarnessConfig::models_path_for(&dir)).unwrap();
        assert!(doc.providers.is_empty());
        assert!(doc.active.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn models_document_roundtrips_with_the_provider_key_names() {
        let dir = temp_dir("models-roundtrip");
        let path = dir.join(".harness").join("models.json");
        let mut doc = ModelsDocument::default();
        doc.upsert_route(
            "default",
            ProviderProfile {
                display_name: Some("DeepSeek".into()),
                api: Some("openai-completions".into()),
                base_url: Some("http://127.0.0.1:8899/v1".into()),
                api_key_env: Some("AI_API_KEY".into()),
                models: Some(vec![ProviderModel {
                    id: "deepseek-chat".into(),
                    name: Some("deepseek-chat (mock)".into()),
                    context_window: Some(131_072),
                    max_tokens: Some(16_384),
                }]),
            },
        );
        doc.active = Some(ActiveRoute {
            provider: "default".into(),
            model: "deepseek-chat".into(),
        });
        doc.save(&path).unwrap();

        // The written bytes must use the provider's own field names, because the
        // provider refuses a document whose fields it does not know.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("\"baseURL\""),
            "provider field name is baseURL: {raw}"
        );
        assert!(
            raw.contains("\"contextWindow\""),
            "provider field name is contextWindow: {raw}"
        );
        assert!(
            !raw.contains("base_url"),
            "snake_case would be refused by the provider"
        );

        let back = ModelsDocument::load(&path).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.routes().len(), 1);
        assert_eq!(back.routes()[0].0, "default");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_document_is_reported_rather_than_ignored() {
        let dir = temp_dir("models-invalid");
        let path = dir.join("models.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(ModelsDocument::load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fresh_settings_carry_no_model_and_the_engine_defaults() {
        let dir = temp_dir("settings");
        let cfg = HarnessConfig::load(&dir);
        assert_eq!(cfg.caps.token_budget_per_session, 400_000);
        assert!(cfg.agent_panel_open);
        let doc = ModelsDocument::load(&HarnessConfig::models_path_for(&dir)).unwrap();
        assert!(
            doc.active.is_none(),
            "the app must not choose a route for the user"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_roundtrip_through_disk() {
        let dir = temp_dir("settings-roundtrip");
        let mut cfg = HarnessConfig::load(&dir);
        cfg.font_size = 15.5;
        cfg.agent_panel_open = false;
        cfg.save(&dir).unwrap();
        let back = HarnessConfig::load(&dir);
        assert_eq!(back, cfg);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dotenv_loads_ai_names_only() {
        let dir = temp_dir("dotenv");
        std::fs::write(
            dir.join(".env"),
            "# comment\nAI_API_KEY=probe-key\nDEEPSEEK_API_KEY=nope\nAI_MODEL=\n",
        )
        .unwrap();
        // Clear first so the assertion is about this file, not the ambient env.
        std::env::remove_var("HARNESS_TEST_DOTENV");
        let applied = load_dotenv(&dir);
        assert!(applied.contains(&"AI_API_KEY".to_string()));
        assert!(
            std::env::var_os("DEEPSEEK_API_KEY").is_none(),
            "the app never reads DEEPSEEK_API_KEY"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_written_credential_replaces_the_named_variable_and_keeps_the_rest() {
        let dir = temp_dir("dotenv-write");
        std::fs::write(dir.join(".env"), "# my key\nAI_API_KEY=old\nAI_MODEL=x\n").unwrap();
        let path = write_dotenv_value(&dir, "AI_API_KEY", "nvapi-new").unwrap();
        assert_eq!(path, dir.join(".env"));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("# my key"), "comments survive: {body}");
        assert!(body.contains("AI_API_KEY=nvapi-new"), "{body}");
        assert!(
            body.contains("AI_MODEL=x"),
            "unrelated names survive: {body}"
        );
        assert!(!body.contains("old"), "the old value is gone: {body}");

        // Writing again replaces in place rather than appending a second line.
        write_dotenv_value(&dir, "AI_API_KEY", "second").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.matches("AI_API_KEY=").count(), 1, "{body}");
        assert!(body.contains("AI_API_KEY=second"), "{body}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_written_credential_creates_the_file_and_refuses_a_name_nothing_reads() {
        let dir = temp_dir("dotenv-create");
        let path = write_dotenv_value(&dir, "AI_API_KEY", "probe").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "AI_API_KEY=probe\n"
        );
        assert!(write_dotenv_value(&dir, "DEEPSEEK_API_KEY", "probe").is_err());
        assert!(write_dotenv_value(&dir, "AI_API_KEY", "  ").is_err());
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            body, "AI_API_KEY=probe\n",
            "a refused pair writes nothing: {body}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
