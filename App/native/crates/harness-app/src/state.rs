//! The application's working state: everything the panes read and write.
//!
//! The state is one struct because the panes are one product. A command typed
//! into the terminal and a prompt sent to the agent both land here; the git pane
//! and the editor look at the same workspace; the meter counts what the agent
//! panel shows. Splitting this into per-pane stores would only mean nine ways to
//! keep them in sync.
//!
//! Nothing here blocks the frame. Subsystems that can take time — the engine's
//! startup, a search, a git read — are either threaded (their events arrive in
//! [`HarnessState::pump`]) or explicitly requested by the pane that needs them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use harness_core::acp::{AcpClient, AgentEvent, AgentRequest, ConfigChoice, PermissionOption};
use harness_core::agent::{Conversation, Item, NoticeLevel, RunState};
use harness_core::config::{
    EnginePaths, HarnessConfig, ModelsDocument, Price, Prices, ProviderModel, ProviderProfile,
};
use harness_core::event::{ConversationId, CoreEvent, TerminalId};
use harness_core::fsops::{self, DirListing, FileText, SearchOptions, SearchOutcome};
use harness_core::git::{BranchInfo, CommitInfo, GitRepo, RepoStatus};
use harness_core::tasks::{TaskRecord, TaskStore, TaskSummary};
use harness_core::term::{
    write_shell_integration, BlockSummary, ScreenSize, Terminal, TerminalOptions,
};
use harness_core::workflow::{Workflow, WorkflowKind, WorkflowStore};

/// How many scrollback lines each terminal keeps.
const SCROLLBACK: usize = 10_000;

/// How many palette paths to walk before giving up on a bigger answer.
const PALETTE_FILES: usize = 20_000;

/// How many blocks a shell keeps for the blocks tab.
const BLOCK_LIST: usize = 300;

/// How many characters of a command's output are carried into a prompt.
const BLOCK_PROMPT_LIMIT: usize = 6_000;

/// Patch lines above which the diff opens collapsed.
const COMPACT_DIFF_LINES: usize = 1_200;

/// How often a task with unsaved changes is written while a turn runs, so a
/// crash mid-turn loses seconds of transcript rather than the turn.
const TASK_SAVE_EVERY: Duration = Duration::from_secs(5);

/// Longest a turn may wait for its session before it is failed outright. The
/// engine's own cold start is a handful of seconds; well past a minute means
/// something is stuck, not merely slow.
const SESSION_START_TIMEOUT: Duration = Duration::from_secs(75);

/// How many back/forward steps the navigation history keeps.
const NAV_DEPTH: usize = 64;

/// How long the skills and MCP lists are trusted before the disk is read again.
const EXTENSIONS_TTL: Duration = Duration::from_secs(15);

/// What the agent can be extended with, as last read from disk.
pub struct Extensions {
    /// Skills a session can load.
    pub skills: Vec<harness_core::extensions::Skill>,
    /// MCP servers the workspace declares, or why its document is unreadable.
    pub mcp: Result<Vec<harness_core::extensions::McpServer>, String>,
    /// When they were read.
    pub read_at: Instant,
}

/// One file the task created or changed, for the panel's artifact list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The path the call named.
    pub path: String,
    /// Lines added across the task.
    pub added: usize,
    /// Lines removed across the task.
    pub removed: usize,
    /// The last call that touched it, to reveal.
    pub call_id: String,
}

/// One place the back and forward arrows can return to: a task and a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    /// The task on screen, `None` for an unsaved new task.
    pub task: Option<String>,
    /// The view in the centre card.
    pub pane: Pane,
}

/// A tool name with its first letter raised, for a panel row.
///
/// @param name the tool's name
/// @returns the name as a label
fn capitalize(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// The first non-empty line of a command, which is how a row shows one.
///
/// @param text the command text
/// @returns one line, trimmed
pub(crate) fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A running turn's age, as the activity row counts it.
///
/// @param elapsed how long the turn has been running
/// @returns seconds under a minute, then minutes and seconds
pub fn elapsed_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

/// Turns an engine failure into a sentence that says what to do about it.
///
/// What arrives is the provider's own error, status code and JSON body and all.
/// A person reading the transcript needs the reason and the way out, and the
/// raw text is still in the log for anyone who wants it.
///
/// @param message the engine's message
/// @returns a sentence for the transcript
pub fn describe_failure(message: &str) -> String {
    let lower = message.to_lowercase();
    let detail = provider_detail(message);
    if lower.contains("free-models-per-day") || lower.contains("openrouter_free_tier_daily") {
        let reset = message.find('{')
            .and_then(|at| serde_json::from_str::<serde_json::Value>(&message[at..]).ok())
            .and_then(|body| body.pointer("/metadata/headers/X-RateLimit-Reset")?.as_str()?.parse::<i64>().ok())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| format!(" The reported reset is {} UTC.", time.format("%Y-%m-%d %H:%M")))
            .unwrap_or_default();
        return format!("the account's daily free-model quota is exhausted (429). Switching free models or keys on the same account will not reset it. Wait for the daily reset, or choose a route with available quota (⌘,).{reset}");
    }
    if lower.contains("invalid url") || lower.contains("base url must") || lower.contains("baseurl must") {
        return "the route's base URL is invalid. Open models and routes (⌘,) and enter the complete API base URL, including https:// and the provider's API path.".to_string();
    }
    if lower.contains("finish_reason: error") {
        return "the provider ended its response with an error and supplied no further detail. This is not evidence of a bad API key. Retry once, or choose another route (⌘,).".to_string();
    }
    if lower.contains("429") || lower.contains("rate-limited") || lower.contains("rate limit") {
        let advice = "the route is rate-limited upstream (429). Try again in a moment, or pick another route (⌘,).";
        return match detail {
            Some(detail) => format!("{advice} The provider said: {detail}"),
            None => advice.to_string(),
        };
    }
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
    {
        return "the provider rejected the key (401). Check the key for this route (⌘,) — a key from one provider does not work for another.".to_string();
    }
    if lower.contains("404") {
        return "the provider answered 404. The base URL or the model id is wrong for this route — the route dialog (⌘,) has both.".to_string();
    }
    if lower.contains("no adapter registered") || lower.contains("needs an api") {
        return "the engine has no adapter for this route. Open ⌘, and give the route an api (openai-completions).".to_string();
    }
    if lower.contains("econnrefused") || lower.contains("fetch failed") || lower.contains("connection") {
        return format!(
            "the provider could not be reached. Check the base URL and the network — {}",
            shorten(message, 160)
        );
    }
    if lower.contains("budget") || lower.contains("ceiling") {
        return message.to_string();
    }
    format!(
        "the turn failed: {}",
        shorten(detail.as_deref().unwrap_or(message), 200)
    )
}

/// The readable part of a provider error: its `raw` hint, or its message.
///
/// @param message the engine's message, JSON body included
/// @returns the human line worth showing, when the body had one
fn provider_detail(message: &str) -> Option<String> {
    let brace = message.find('{')?;
    let body: serde_json::Value = serde_json::from_str(message[brace..].trim()).ok()?;
    let text = body
        .pointer("/metadata/raw")
        .and_then(|value| value.as_str())
        .or_else(|| body.get("message").and_then(|value| value.as_str()))?;
    Some(shorten(text, 180))
}

/// Cuts a line to a length a panel can hold, marked when cut.
///
/// @param text the line
/// @param limit the most characters to keep
/// @returns the line, elided at the end
fn shorten(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// The tail of a reply, ending on a line boundary and marked when cut.
///
/// @param text the reply
/// @param limit the most characters to keep
/// @returns the text, elided at the front
fn prose_tail(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    match tail.find(' ') {
        Some(cut) => format!("…{}", tail[cut + 1..].trim_start()),
        None => format!("…{tail}"),
    }
}

/// An answer's opening as plain sentences, for the panel's summary card.
///
/// The card is a glance at what the agent last said, so it keeps the prose and
/// drops what only reads rendered: fenced blocks, table rows, heading and list
/// markers, emphasis, and inline-code ticks.
///
/// @param text the answer, in markdown
/// @param limit the most characters to keep
/// @returns the summary
pub fn plain_summary(text: &str, limit: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed.is_empty() || trimmed.starts_with('|') || trimmed.starts_with("---") {
            continue;
        }
        let heading = trimmed.starts_with('#');
        let bare = trimmed
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '+', '>'])
            .trim_start();
        let bare = bare
            .split_once(". ")
            .filter(|(number, _)| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, rest)| rest)
            .unwrap_or(bare);
        let mut line = bare.replace("**", "").replace('`', "");
        // A heading is a sentence of its own once the markers are gone.
        if heading && !line.ends_with(['.', ':', '!', '?']) {
            line.push('.');
        }
        out.push(line);
        if out.iter().map(String::len).sum::<usize>() > limit {
            break;
        }
    }
    let joined = out.join(" ");
    if joined.chars().count() <= limit {
        return joined;
    }
    let cut: String = joined.chars().take(limit).collect();
    match cut.rfind(' ') {
        Some(space) => format!("{}…", &cut[..space]),
        None => format!("{cut}…"),
    }
}

/// Clips text to a character budget, on a line boundary when there is one.
///
/// @param text the text
/// @param limit the most characters to keep
/// @returns the text, cut at the last line break inside the budget
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    let cut = head.rfind('\n').unwrap_or(head.len());
    head[..cut].to_string()
}

/// Which view the centre card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The conversation with the engine: the product's home view.
    Conversation,
    /// The PTY-backed terminal and its blocks.
    Terminal,
    /// The editor.
    Editor,
    /// Repository state and diffs.
    Git,
    /// Saved commands, prompts, and notes.
    Workflows,
    /// Token, context, and tool observability.
    Observability,
}

impl Pane {
    /// Every view, in switcher order.
    pub const ALL: [Pane; 6] = [
        Pane::Conversation,
        Pane::Terminal,
        Pane::Editor,
        Pane::Git,
        Pane::Workflows,
        Pane::Observability,
    ];

    /// The switcher's label.
    ///
    /// @returns a short name
    pub fn label(self) -> &'static str {
        match self {
            Self::Conversation => "CONVO",
            Self::Terminal => "TERM",
            Self::Editor => "CODE",
            Self::Git => "GIT",
            Self::Workflows => "FLOW",
            Self::Observability => "METER",
        }
    }

    /// The view's full name, for titles and the palette.
    ///
    /// @returns the name shown to the user
    pub fn title(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Terminal => "Terminal",
            Self::Editor => "Editor",
            Self::Git => "Git",
            Self::Workflows => "Workflows",
            Self::Observability => "Observability",
        }
    }

    /// The view a name refers to, by long name or short label.
    ///
    /// @param name the name, in any case
    /// @returns the view, when the name is one
    pub fn from_name(name: &str) -> Option<Self> {
        let wanted = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|pane| {
            pane.title().eq_ignore_ascii_case(&wanted) || pane.label().eq_ignore_ascii_case(&wanted)
        })
    }
}

/// Which row of the sidebar is the current one.
///
/// The rows are named the way the reference client names them; each one opens
/// the view in this harness that does the same job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarSpot {
    /// The task row under the workspace: the conversation.
    Task,
    /// Recent work: saved workflows and notes.
    Knowledge,
    /// Files: the editor.
    Sites,
    /// What ran unattended: the observability meter.
    Automations,
    /// Model routes and plugins.
    Extensions,
}

/// One delegation the engine started, as the panel lists it.
#[derive(Debug, Clone)]
pub struct Delegation {
    /// The agent the work was handed to.
    pub agent: String,
    /// The task's short description.
    pub task: String,
    /// The tool call it belongs to, so the panel can jump to it.
    pub call_id: String,
    /// Whether the run has settled.
    pub done: bool,
    /// Whether it settled badly: a failed call, or a part reported failed.
    pub failed: bool,
}

/// Where a background process row comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessSource {
    /// A tool call the engine made, by call id.
    Tool(String),
    /// One of the user's own shells, by index.
    Shell(usize),
}

/// One command in flight, or one that has finished, as the panel lists it.
#[derive(Debug, Clone)]
pub struct Process {
    /// The command line, or what is known of it.
    pub command: String,
    /// The word the row ends with: running, done, waiting, or idle.
    pub state: String,
    /// Whether it is still running.
    pub running: bool,
    /// What to open when the row is clicked.
    pub source: ProcessSource,
}

/// What the session is doing right now, for the activity line.
///
/// A turn can run for minutes without a single visible change: the engine is
/// thinking, or a command is running inside a tool call. This is the app's
/// answer to "is it still there?" — derived from the transcript rather than
/// announced by the engine, so it is always available while a prompt runs.
#[derive(Debug, Clone)]
pub struct Activity {
    /// One line naming the work.
    pub headline: String,
    /// The most specific thing known: a command, a call's own words.
    pub detail: Option<String>,
    /// How long the current turn has been running.
    pub elapsed: Option<Duration>,
}

/// Which tab the terminal pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTab {
    /// The live emulator grid.
    Screen,
    /// The finished commands and their outputs.
    Blocks,
}

/// A field the keyboard or a button asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusRequest {
    /// The agent's prompt box.
    Agent,
    /// The palette's query field.
    Palette,
    /// The content search's field.
    Search,
}

/// One shell session.
pub struct Shell {
    /// Terminal identity, shared with the core's events.
    pub id: TerminalId,
    /// The PTY, once it started.
    pub terminal: Option<Terminal>,
    /// Why it is not running, when it is not.
    pub error: Option<String>,
    /// The name in the tab strip.
    pub name: String,
    /// Which tab is showing.
    pub tab: ShellTab,
    /// The block the blocks tab is showing.
    pub selected_block: Option<usize>,
    /// Finished commands, oldest first, as the blocks tab lists them.
    pub blocks: Vec<BlockSummary>,
    /// Whether a command finished since the list was read.
    pub blocks_dirty: bool,
    /// Whether the screen follows new output.
    pub follow: bool,
}

impl Shell {
    /// A shell that has not started yet.
    ///
    /// @param id terminal identity
    /// @param name tab label
    /// @returns the shell
    fn new(id: TerminalId, name: String) -> Self {
        Self {
            id,
            terminal: None,
            error: None,
            name,
            tab: ShellTab::Screen,
            selected_block: None,
            blocks: Vec::new(),
            blocks_dirty: true,
            follow: true,
        }
    }

    /// Whether the shell's process is alive.
    ///
    /// @returns true while the shell runs
    pub fn alive(&self) -> bool {
        self.terminal.as_ref().is_some_and(Terminal::is_alive)
    }
}

/// One open file in the editor.
pub struct Buffer {
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the workspace root.
    pub relative: String,
    /// The current text.
    pub text: String,
    /// The text as it was read or last written.
    pub saved: String,
    /// Size on disk when it was read.
    pub bytes: u64,
    /// Whether the read stopped at the load limit.
    pub truncated: bool,
    /// Whether the file is binary rather than text.
    pub binary: bool,
    /// Language id for highlighting.
    pub language: &'static str,
    /// Whether the pane is in edit mode rather than the read-only view.
    pub editing: bool,
    /// Where the caret was, so reopening a file does not lose the place.
    pub scroll_line: Option<usize>,
}

impl Buffer {
    /// Whether the buffer has unsaved edits.
    ///
    /// @returns true when the text differs from what was read
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }
}

/// The file tree's expansion state and cached listings.
pub struct TreeState {
    /// Directories the user opened.
    pub expanded: HashSet<PathBuf>,
    /// Listings, keyed by directory.
    pub listings: HashMap<PathBuf, DirListing>,
    /// Whether ignored paths are shown, dimmed.
    pub show_ignored: bool,
    /// What the tree's filter field holds; files are hidden when they miss it.
    pub filter: String,
}

impl TreeState {
    /// An empty tree.
    ///
    /// @returns the state
    fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            listings: HashMap::new(),
            show_ignored: false,
            filter: String::new(),
        }
    }

    /// Forgets every cached listing, so the next frame re-reads them.
    pub fn invalidate(&mut self) {
        self.listings.clear();
    }

    /// Reads a directory into the cache.
    ///
    /// @param root workspace root
    /// @param dir directory to read
    /// @returns the listing, or the error it produced
    pub fn ensure(&mut self, root: &Path, dir: &Path) -> Option<&DirListing> {
        if !self.listings.contains_key(dir) {
            match fsops::list_dir(root, dir, self.show_ignored) {
                Ok(listing) => {
                    self.listings.insert(dir.to_path_buf(), listing);
                }
                Err(err) => {
                    log::warn!("tree: {err}");
                    // An unreadable directory is remembered as empty rather than
                    // re-read every frame.
                    self.listings.insert(
                        dir.to_path_buf(),
                        DirListing {
                            relative: String::new(),
                            entries: Vec::new(),
                        },
                    );
                }
            }
        }
        self.listings.get(dir)
    }
}

/// The editor's state.
pub struct EditorState {
    /// The open buffer, when a file is open.
    pub buffer: Option<Buffer>,
    /// The last save error, shown under the toolbar.
    pub error: Option<String>,
    /// A byte offset the next frame should scroll to, from a search hit.
    pub goto: Option<usize>,
}

impl EditorState {
    /// No file open.
    ///
    /// @returns the state
    fn new() -> Self {
        Self {
            buffer: None,
            error: None,
            goto: None,
        }
    }
}

/// The git pane's cached reads.
pub struct GitView {
    /// Working-tree status.
    pub status: RepoStatus,
    /// Local branches.
    pub branches: Vec<BranchInfo>,
    /// Recent commits.
    pub log: Vec<CommitInfo>,
    /// The selected path.
    pub selected: Option<String>,
    /// Whether the selection is the staged side.
    pub staged: bool,
    /// The selected path's patch.
    pub diff: Option<(String, String)>,
    /// Whether the patch is drawn collapsed: file headers, hunk headers, and
    /// the changes with bracketing context rather than every line.
    pub compact: bool,
    /// The commit message being typed.
    pub message: String,
    /// The last git error, shown in the pane.
    pub error: Option<String>,
    /// Whether a read has happened yet.
    pub loaded: bool,
}

impl Default for GitView {
    fn default() -> Self {
        Self {
            status: RepoStatus::default(),
            branches: Vec::new(),
            log: Vec::new(),
            selected: None,
            staged: false,
            diff: None,
            compact: false,
            message: String::new(),
            error: None,
            loaded: false,
        }
    }
}

/// The content search's state.
///
/// The search runs on its own thread: walking a repository is not something a
/// frame may wait for. Results carry the generation they were asked for, so a
/// query typed over an older one arrives and is dropped.
pub struct SearchState {
    /// The pattern.
    pub query: String,
    /// How to match.
    pub options: SearchOptions,
    /// The last result for the current pattern.
    pub outcome: Option<SearchOutcome>,
    /// Whether a search is in flight.
    pub pending: bool,
    /// The pattern the current result belongs to.
    pub query_of_outcome: String,
    /// Monotonic request counter.
    generation: u64,
    /// Results from the search threads.
    results: Receiver<(u64, String, SearchOutcome)>,
    /// The sink those threads publish into.
    sink: Sender<(u64, String, SearchOutcome)>,
}

impl Default for SearchState {
    fn default() -> Self {
        let (sink, results) = crossbeam_channel::unbounded();
        Self {
            query: String::new(),
            options: SearchOptions::default(),
            outcome: None,
            pending: false,
            query_of_outcome: String::new(),
            generation: 0,
            results,
            sink,
        }
    }
}

/// A permission ask the engine is waiting on.
pub struct PermissionAsk {
    /// The JSON-RPC id to answer.
    pub rpc_id: serde_json::Value,
    /// The tool call's title.
    pub title: String,
    /// Why the engine asked, when it said.
    pub reason: Option<String>,
    /// The options the engine offered.
    pub options: Vec<PermissionOption>,
}

/// The agent session, its engine process, and the pending asks.
///
/// The engine's channel is a standard one rather than a crossbeam one because
/// the ACP client owns its receiving half's type; the core's terminal events
/// stay on crossbeam, where the sink is shared.
pub struct AgentState {
    /// Sink the engine's reader threads publish into.
    pub sender: std::sync::mpsc::Sender<AgentEvent>,
    /// The app's end of that channel.
    pub events: std::sync::mpsc::Receiver<AgentEvent>,
    /// The engine process, once started.
    pub client: Option<AcpClient>,
    /// The transcript, run state, and meter.
    pub conversation: Conversation,
    /// Permission asks awaiting an answer.
    pub pending: Vec<PermissionAsk>,
    /// The prompt box's text.
    pub input: String,
    /// A prompt typed before the session existed, sent once it does.
    pub pending_input: Option<String>,
    /// The last prompt that was sent, kept so a failure can be retried.
    pub last_prompt: Option<String>,
    /// Whether a start is in flight.
    pub starting: bool,
    /// Why the engine could not start.
    pub start_error: Option<String>,
    /// The route the session should run on, `route/model`.
    pub requested_route: Option<String>,
    /// The model the engine says the session is using.
    pub applied_model: Option<String>,
    /// Whether permission asks are answered with their first allow option
    /// without stopping for the user.
    pub auto_approve: bool,
    /// A question the agent is being asked to answer from the terminal.
    pub attached_block: Option<String>,
    /// A saved session to reopen as soon as the engine is ready, instead of a
    /// fresh one — set when a task is opened from history before the engine
    /// is up.
    pub resume: Option<String>,
    /// Sessions this engine process has open, so switching back to a task
    /// whose session is still live needs no resume.
    pub live_sessions: HashSet<String>,
    /// The requested route a restart has already been tried for, so a route
    /// this engine process never ends up serving restarts once and then
    /// reports the mismatch instead of restarting forever.
    pub route_restart_attempted: Option<String>,
}

/// The editor for a saved workflow.
pub struct WorkflowDraft {
    /// The workflow being edited, or a new one.
    pub workflow: Workflow,
    /// Whether it is new rather than a copy of an existing one.
    pub is_new: bool,
    /// Placeholder values being typed.
    pub values: BTreeMap<String, String>,
    /// Tags as one comma-separated line, which is how a form edits a list.
    pub tags: String,
}

/// The model-route editor.
pub struct ModelDraft {
    /// Route key, e.g. `deepseek`.
    pub route: String,
    /// Human label.
    pub display_name: String,
    /// API root.
    pub base_url: String,
    /// Environment variable holding the credential.
    pub api_key_env: String,
    /// The credential being set, when the user is setting one. It is written to
    /// the workspace `.env` — never to the route document, and never read back
    /// into this field once saved.
    pub api_key: String,
    /// One model per line: `id | name | context | max tokens`.
    pub models: String,
    /// Which model the session should start on.
    pub active: String,
    /// Input price per million tokens.
    pub price_input: String,
    /// Output price per million tokens.
    pub price_output: String,
    /// Whether this route already existed when the editor opened.
    pub existing: bool,
}

impl ModelDraft {
    /// An empty draft: a route someone is about to describe.
    ///
    /// @returns the draft
    pub fn empty() -> Self {
        Self {
            route: String::new(),
            display_name: String::new(),
            base_url: String::new(),
            api_key_env: "AI_API_KEY".to_string(),
            api_key: String::new(),
            models: String::new(),
            active: String::new(),
            price_input: String::new(),
            price_output: String::new(),
            existing: false,
        }
    }
}

/// A note being edited.
pub struct NoteDraft {
    /// File name, without the directory.
    pub name: String,
    /// The note's text.
    pub text: String,
    /// The name it was loaded from, when it is not new.
    pub original: Option<String>,
}

/// The folder picker: the workspace the app is about to open.
///
/// The picker browses the filesystem itself rather than calling a native dialog,
/// so what it can do is exactly what is drawn — and so a frame capture can show
/// it. The typed field and the list are the same thing: entering a row writes the
/// path back into the field, and typing a path and pressing enter moves the list.
pub struct FolderDraft {
    /// The directory being browsed.
    pub dir: PathBuf,
    /// The path being typed, which is also the browsed one.
    pub typed: String,
    /// Why the last move failed, when one did.
    pub error: Option<String>,
}

impl FolderDraft {
    /// Opens the picker on a directory.
    ///
    /// @param dir where to start
    /// @returns the draft
    pub fn new(dir: PathBuf) -> Self {
        let typed = dir.display().to_string();
        Self {
            dir,
            typed,
            error: None,
        }
    }

    /// Moves the picker to a path, when it is a directory.
    ///
    /// @param path where to go
    pub fn enter(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.typed = path.display().to_string();
            self.dir = path;
            self.error = None;
        } else {
            self.error = Some(format!("{} is not a directory", path.display()));
        }
    }

    /// Moves up to the parent directory.
    pub fn up(&mut self) {
        match self.dir.parent() {
            Some(parent) => self.enter(parent.to_path_buf()),
            None => self.error = Some("this is the top of the filesystem".to_string()),
        }
    }

    /// The subdirectories of the browsed directory, hidden ones dimmed.
    ///
    /// Files are left out on purpose: a workspace is a folder, and a row that
    /// cannot be opened would only be noise in a list this narrow.
    ///
    /// @returns `(name, path, hidden)` rows, or why the directory could not be read
    pub fn dirs(&self) -> Result<Vec<(String, PathBuf, bool)>, String> {
        let listing = fsops::list_dir(&self.dir, &self.dir, true)?;
        Ok(listing
            .entries
            .into_iter()
            .filter(|entry| entry.is_dir)
            .map(|entry| {
                let hidden = entry.name.starts_with('.');
                (entry.name, entry.path, hidden)
            })
            .collect())
    }
}

/// Where the app can jump to in the folder picker.
///
/// Only places that exist are offered, so every chip does something.
///
/// @param current the workspace already open
/// @param repo the repository root, when one resolved
/// @returns `(label, path)` shortcuts
pub fn folder_shortcuts(current: &Path, repo: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mut shortcuts = vec![("current workspace".to_string(), current.to_path_buf())];
    if let Some(repo) = repo {
        if repo != current {
            shortcuts.push(("harness repo".to_string(), repo.to_path_buf()));
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for (label, candidate) in [
            ("home", home.clone()),
            ("Desktop", home.join("Desktop")),
            ("Documents", home.join("Documents")),
        ] {
            if candidate.is_dir() && candidate != current {
                shortcuts.push((label.to_string(), candidate));
            }
        }
    }
    shortcuts
}

/// Which dialog is open.
pub enum Dialog {
    /// Add or edit a model route.
    Models(ModelDraft),
    /// Add or edit a workflow.
    Workflow(Box<WorkflowDraft>),
    /// Edit a note.
    Note(NoteDraft),
    /// Open a different folder.
    Folder(Box<FolderDraft>),
    /// What this app is, and what it will not do.
    About,
}

/// One hit in the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteHit {
    /// The main line.
    pub label: String,
    /// The dim second line.
    pub detail: String,
    /// What choosing it does.
    pub action: PaletteAction,
}

/// What a palette hit does when chosen.
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteAction {
    /// Open a file in the editor.
    OpenFile(PathBuf),
    /// Open a search hit at a line.
    OpenAt(PathBuf, u64),
    /// Switch the centre pane.
    Pane(Pane),
    /// Start another shell.
    NewShell,
    /// Type a command into the focused terminal.
    RunCommand(String),
    /// Put a prompt in the agent's box and focus it.
    Prompt(String),
    /// Open a workflow, so its placeholders can be filled in before it runs.
    Workflow(String),
    /// Open the palette in another mode.
    Palette(PaletteMode),
    /// Open a dialog.
    Dialog(DialogKind),
    /// Toggle a panel.
    ToggleAgent,
    /// Toggle the tree.
    ToggleTree,
    /// Save the open buffer.
    SaveBuffer,
    /// Export the transcript.
    ExportTranscript,
}

/// Which dialog a palette command opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// The model-route editor.
    Models,
    /// A new workflow.
    Workflow,
    /// A new note.
    Note,
    /// The folder picker.
    Folder,
    /// The about panel.
    About,
}

/// Which kind of search the palette is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Commands.
    Commands,
    /// Files by name.
    Files,
    /// File contents.
    Contents,
}

impl PaletteMode {
    /// The mode after this one, for `tab`.
    ///
    /// @returns the next mode
    pub fn next(self) -> Self {
        match self {
            Self::Commands => Self::Files,
            Self::Files => Self::Contents,
            Self::Contents => Self::Commands,
        }
    }
}

/// The palette's state.
#[derive(Debug, Clone)]
pub struct Palette {
    /// What is being searched.
    pub mode: PaletteMode,
    /// What has been typed.
    pub query: String,
    /// The highlighted hit.
    pub selection: usize,
    /// The hits for the current query.
    pub hits: Vec<PaletteHit>,
    /// Whether the hits need recomputing.
    pub dirty: bool,
}

impl Palette {
    /// A palette in one mode.
    ///
    /// @param mode what to search
    /// @returns the state
    pub fn new(mode: PaletteMode) -> Self {
        Self {
            mode,
            query: String::new(),
            selection: 0,
            hits: Vec::new(),
            dirty: true,
        }
    }
}

/// Everything the interface reads and writes.
pub struct HarnessState {
    /// Workspace root.
    pub root: PathBuf,
    /// Harness home the caller named, so a switch re-resolves the same layout.
    pub dsh_home: Option<PathBuf>,
    /// This application's settings for the workspace.
    pub config: HarnessConfig,
    /// The route registry the engine reads.
    pub models: ModelsDocument,
    /// At most one route is active; this is it, as `route/model`.
    pub active_route: Option<String>,
    /// Token prices, when the user has entered them.
    pub prices: Prices,
    /// Where the engine lives, when it was found.
    pub paths: Result<EnginePaths, String>,
    /// The repository, when the workspace is in one.
    pub git: Option<GitRepo>,
    /// The file tree.
    pub tree: TreeState,
    /// Paths for the palette's quick open, refreshed on request.
    pub palette_files: Option<Vec<String>>,
    /// The editor.
    pub editor: EditorState,
    /// The content search.
    pub search: SearchState,
    /// The git pane.
    pub git_view: GitView,
    /// Open shells.
    pub shells: Vec<Shell>,
    /// Which shell the terminal pane shows.
    pub active_shell: usize,
    /// The agent session.
    pub agent: AgentState,
    /// Saved workflows.
    pub workflows: WorkflowStore,
    /// Note files under `.harness/notes`.
    pub notes: Vec<PathBuf>,
    /// Which view the centre column shows.
    pub pane: Pane,
    /// Which sidebar row is current.
    pub spot: SidebarSpot,
    /// When something last happened, for the panel's notice line.
    pub last_activity: chrono::DateTime<chrono::Local>,
    /// A tool call the panel asked the conversation to open and scroll to.
    pub reveal_call: Option<String>,
    /// Whether the file tree is open.
    pub tree_open: bool,
    /// Whether the agent panel is open.
    pub agent_open: bool,
    /// The open dialog, if any.
    pub dialog: Option<Dialog>,
    /// The command palette, if open.
    pub palette: Option<Palette>,
    /// A field the keyboard asked for, consumed by the pane that owns it.
    pub focus: Option<FocusRequest>,
    /// A transient message, with when it arrived.
    pub toast: Option<(String, Instant)>,
    /// Whether the key reference is open.
    pub help_open: bool,
    /// Core events from the terminal subsystem.
    pub core: Receiver<CoreEvent>,
    /// The sink the core publishes into.
    pub sink: Sender<CoreEvent>,
    /// Monotonic terminal ids.
    next_terminal: u64,
    /// The workspace's saved tasks, newest first.
    pub tasks: Vec<TaskSummary>,
    /// The task on screen, once it has been saved (after its first prompt).
    pub current_task: Option<String>,
    /// When the task on screen began.
    task_created: Option<String>,
    /// The name the user gave the task on screen, when they renamed it.
    task_title: Option<String>,
    /// Whether the transcript changed since the task was last written.
    task_dirty: bool,
    /// When the task was last written.
    task_saved_at: Instant,
    /// Whether the sidebar is expanded.
    pub sidebar_open: bool,
    /// Where back and forward go.
    pub nav: Vec<NavEntry>,
    /// The entry on screen, an index into [`HarnessState::nav`].
    pub nav_index: usize,
    /// A task being renamed in place: its id and the name being typed.
    pub renaming: Option<(String, String)>,
    /// Skills and MCP servers, cached so the panel does not scan disks per frame.
    pub extensions: Extensions,
    /// Parsed-markdown cache for the transcript's answers.
    pub markdown: egui_commonmark::CommonMarkCache,
    /// Set when the transcript should jump to its newest line on the next frame.
    pub scroll_to_bottom: bool,
}

impl HarnessState {
    /// Builds the state for a workspace.
    ///
    /// @param root workspace root
    /// @param dsh_home explicit Harness home, when the caller named one
    /// @returns the state, with the first shell started
    pub fn new(root: PathBuf, dsh_home: Option<PathBuf>) -> Self {
        let config = HarnessConfig::load(&root);
        let models =
            ModelsDocument::load(&HarnessConfig::models_path_for(&root)).unwrap_or_else(|err| {
                log::error!("models: {err}");
                ModelsDocument::default()
            });
        let prices = Prices::load(&HarnessConfig::prices_path_for(&root)).unwrap_or_default();
        let active_route = models
            .active
            .as_ref()
            .map(|route| format!("{}/{}", route.provider, route.model));
        let (sink, core) = crossbeam_channel::unbounded();
        // The engine's events arrive on a standard channel: that is the one the
        // ACP client was given, and one receiver for the whole app is enough.
        let (sender, events) = std::sync::mpsc::channel();
        let paths = EnginePaths::resolve(&root, dsh_home.clone());
        if let Err(err) = &paths {
            log::warn!("engine: {err}");
        }
        let workflows = WorkflowStore::load(&HarnessConfig::workflows_dir_for(&root));
        let git = GitRepo::discover(&root);

        let mut state = Self {
            config: config.clone(),
            models,
            active_route,
            prices,
            paths,
            git,
            tree: TreeState::new(),
            palette_files: None,
            editor: EditorState::new(),
            search: SearchState::default(),
            git_view: GitView::default(),
            shells: Vec::new(),
            active_shell: 0,
            agent: AgentState {
                sender,
                events,
                client: None,
                conversation: Conversation::new(ConversationId(1)),
                pending: Vec::new(),
                input: String::new(),
                pending_input: None,
                last_prompt: None,
                starting: false,
                start_error: None,
                requested_route: None,
                applied_model: None,
                auto_approve: false,
                attached_block: None,
                resume: None,
                live_sessions: HashSet::new(),
                route_restart_attempted: None,
            },
            workflows,
            notes: Vec::new(),
            pane: Pane::Conversation,
            spot: SidebarSpot::Task,
            last_activity: chrono::Local::now(),
            reveal_call: None,
            tree_open: config.tree_open,
            agent_open: config.agent_panel_open,
            dialog: None,
            palette: None,
            focus: None,
            toast: None,
            help_open: false,
            core,
            sink,
            next_terminal: 1,
            tasks: TaskStore::for_workspace(&root).list(),
            current_task: None,
            task_created: None,
            task_title: None,
            task_dirty: false,
            task_saved_at: Instant::now(),
            sidebar_open: config.sidebar_open,
            nav: vec![NavEntry {
                task: None,
                pane: Pane::Conversation,
            }],
            nav_index: 0,
            renaming: None,
            extensions: Extensions {
                skills: Vec::new(),
                mcp: Ok(Vec::new()),
                // Stale from the start, so the first frame that asks reads them.
                read_at: Instant::now() - EXTENSIONS_TTL * 2,
            },
            markdown: egui_commonmark::CommonMarkCache::default(),
            scroll_to_bottom: false,
            root,
            dsh_home,
        };
        state.refresh_notes();
        state.new_shell();
        state
    }

    /// The task files of the open workspace.
    ///
    /// @returns the store
    pub fn task_store(&self) -> TaskStore {
        TaskStore::for_workspace(&self.root)
    }

    /// Skills and MCP servers, re-read when the cached copy is old.
    ///
    /// @returns the current lists
    pub fn extensions(&mut self) -> &Extensions {
        if self.extensions.read_at.elapsed() >= EXTENSIONS_TTL {
            let home = self.paths.as_ref().ok().map(|paths| paths.dsh_home.clone());
            self.extensions = Extensions {
                skills: harness_core::extensions::skills(&self.root, home.as_deref()),
                mcp: harness_core::extensions::mcp_servers(&self.root),
                read_at: Instant::now(),
            };
        }
        &self.extensions
    }

    /// The files this task created or changed, in the order first touched.
    ///
    /// @returns one row per path, with the lines added and removed across the task
    pub fn artifacts(&self) -> Vec<Artifact> {
        let mut rows: Vec<Artifact> = Vec::new();
        for item in &self.agent.conversation.items {
            let Item::Tool(card) = item else { continue };
            // A wave's parts were built by its children, whose own calls never
            // reach this transcript: the files each finished part owned are what
            // it produced.
            for part in wave_parts(card).into_iter().filter(|part| part.done && !part.failed) {
                for file in wave_files(card, &part.agent) {
                    if !rows.iter().any(|row| row.path == file) {
                        rows.push(Artifact {
                            path: file,
                            added: 0,
                            removed: 0,
                            call_id: card.id.clone(),
                        });
                    }
                }
            }
            let Some(change) = card.change().filter(|change| !change.path.is_empty()) else {
                continue;
            };
            match rows.iter_mut().find(|row| row.path == change.path) {
                Some(row) => {
                    row.added += change.added.len();
                    row.removed += change.removed.len();
                    row.call_id = card.id.clone();
                }
                None => rows.push(Artifact {
                    path: change.path.clone(),
                    added: change.added.len(),
                    removed: change.removed.len(),
                    call_id: card.id.clone(),
                }),
            }
        }
        rows
    }

    /// Stops the running turn.
    ///
    /// The engine settles the turn itself (`cancelled`), which is what ends the
    /// run state; saying so here keeps the transcript honest in between.
    pub fn stop_turn(&mut self) {
        if !self.agent.conversation.state.busy() {
            return;
        }
        match (
            self.agent.client.as_ref(),
            self.agent.conversation.session_id.as_ref(),
        ) {
            (Some(client), Some(session)) => {
                client.cancel(session);
                self.agent
                    .conversation
                    .notice(NoticeLevel::Info, "stopping — the engine is ending the turn");
            }
            _ => self.agent.conversation.state = RunState::Idle,
        }
        self.task_dirty = true;
    }

    /// The workspace's MCP servers in ACP's list form, for a session request.
    ///
    /// A document that does not parse sends no servers and says why once in
    /// the log; a session without its tools beats no session at all.
    ///
    /// @returns the servers to attach
    pub fn mcp_wire(&self) -> Vec<serde_json::Value> {
        match harness_core::extensions::mcp_servers(&self.root) {
            Ok(servers) => servers.into_iter().map(|server| server.wire).collect(),
            Err(err) => {
                log::warn!("mcp: {err}");
                Vec::new()
            }
        }
    }

    /// Re-reads the workspace's task list.
    pub fn refresh_tasks(&mut self) {
        self.tasks = self.task_store().list();
    }

    /// Writes the task on screen to its file, when there is anything to keep.
    ///
    /// A task exists from its first prompt: a conversation with nothing asked
    /// is not history. The name follows the first prompt until the user renames
    /// the task, and the engine session id rides along so reopening the task
    /// resumes the model's context rather than starting cold.
    pub fn save_task(&mut self) {
        let Some(first) = self.agent.conversation.first_prompt().map(str::to_string) else {
            return;
        };
        let now = chrono::Local::now().to_rfc3339();
        let id = self
            .current_task
            .get_or_insert_with(TaskStore::new_id)
            .clone();
        let created = self.task_created.get_or_insert_with(|| now.clone()).clone();
        let renamed = self.task_title.is_some();
        let record = TaskRecord {
            id,
            title: self
                .task_title
                .clone()
                .unwrap_or_else(|| TaskStore::title_for(&first)),
            renamed,
            created,
            updated: now,
            session_id: self.agent.conversation.session_id.clone(),
            route: self.active_route.clone(),
            items: self.agent.conversation.items.clone(),
        };
        match self.task_store().save(&record) {
            Ok(()) => {
                self.task_dirty = false;
                self.task_saved_at = Instant::now();
                self.refresh_tasks();
            }
            Err(err) => log::warn!("tasks: {err}"),
        }
    }

    /// Opens a saved task: its transcript on screen, and its engine session
    /// resumed so the next prompt continues with the model's full context.
    ///
    /// A turn still running in the task being left is cancelled first: its
    /// updates would otherwise stream into a transcript nobody is looking at.
    ///
    /// @param id the task to open
    pub fn open_task(&mut self, id: &str) {
        if self.current_task.as_deref() == Some(id) {
            self.pane = Pane::Conversation;
            self.spot = SidebarSpot::Task;
            return;
        }
        let record = match self.task_store().load(id) {
            Ok(record) => record,
            Err(err) => {
                self.toast(format!("could not open that task: {err}"));
                self.refresh_tasks();
                return;
            }
        };
        self.leave_task();
        let next = ConversationId(self.agent.conversation.id.0 + 1);
        let mut conversation = Conversation::restore(next, None, record.items);
        conversation.engine_name = self.agent.conversation.engine_name.clone();
        conversation.engine_version = self.agent.conversation.engine_version.clone();
        self.agent.conversation = conversation;
        self.current_task = Some(record.id.clone());
        self.task_created = Some(record.created);
        self.task_title = record.renamed.then_some(record.title);
        self.task_dirty = false;
        self.pane = Pane::Conversation;
        self.spot = SidebarSpot::Task;
        self.last_activity = chrono::Local::now();
        self.scroll_to_bottom = true;
        self.agent.last_prompt = self
            .agent
            .conversation
            .items
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::User { text, .. } => Some(text.clone()),
                _ => None,
            });
        match record.session_id {
            Some(session) if self.agent.live_sessions.contains(&session) => {
                // Still open in this engine: nothing to reopen.
                self.agent.conversation.session_id = Some(session);
            }
            Some(session) => match self.agent.client.as_ref() {
                Some(client) => {
                    let mcp = self.mcp_wire();
                    client.resume_session(&session, &self.root.to_string_lossy(), &mcp);
                }
                None => {
                    self.agent.resume = Some(session);
                    self.ensure_engine();
                }
            },
            None => self.open_session(),
        }
        self.record_nav();
    }

    /// Saves the task on screen and stops anything it still has running.
    fn leave_task(&mut self) {
        if self.agent.conversation.state.busy() {
            if let (Some(client), Some(session)) = (
                self.agent.client.as_ref(),
                self.agent.conversation.session_id.as_ref(),
            ) {
                client.cancel(session);
            }
            self.agent
                .conversation
                .notice(NoticeLevel::Warn, "stopped: another task was opened");
            self.agent.conversation.state = RunState::Idle;
        }
        self.save_task();
        self.agent.pending.clear();
        self.agent.input.clear();
        self.agent.pending_input = None;
        self.agent.last_prompt = None;
        self.agent.attached_block = None;
        self.agent.resume = None;
        self.reveal_call = None;
        self.renaming = None;
    }

    /// Opens a fresh engine session for the conversation on screen.
    fn open_session(&mut self) {
        match (self.agent.client.as_ref(), self.agent.starting) {
            (Some(client), _) => {
                client.new_session_with(&self.root.to_string_lossy(), &self.mcp_wire());
            }
            (None, false) => self.ensure_engine(),
            (None, true) => {}
        }
    }

    /// Renames a saved task, or the task on screen.
    ///
    /// @param id the task
    /// @param title the new name
    pub fn rename_task(&mut self, id: &str, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            self.toast("a task needs a name");
            return;
        }
        if self.current_task.as_deref() == Some(id) {
            self.task_title = Some(title.to_string());
            self.save_task();
            return;
        }
        if let Err(err) = self.task_store().rename(id, title) {
            self.toast(err);
        }
        self.refresh_tasks();
    }

    /// Deletes a saved task; deleting the one on screen starts a new task.
    ///
    /// @param id the task
    pub fn delete_task(&mut self, id: &str) {
        if let Err(err) = self.task_store().delete(id) {
            self.toast(err);
        }
        if self.current_task.as_deref() == Some(id) {
            // Nothing of the deleted task may be written back by the switch.
            self.current_task = None;
            self.agent.conversation.items.clear();
            self.new_task();
        }
        self.nav.retain(|entry| entry.task.as_deref() != Some(id));
        if self.nav.is_empty() {
            self.nav.push(NavEntry {
                task: self.current_task.clone(),
                pane: self.pane,
            });
        }
        self.nav_index = self.nav_index.min(self.nav.len() - 1);
        self.refresh_tasks();
        self.toast("task deleted");
    }

    /// Records where the window is now, for back and forward.
    ///
    /// Moving somewhere new after going back drops the entries ahead, the way
    /// a browser's history does.
    pub fn record_nav(&mut self) {
        let entry = NavEntry {
            task: self.current_task.clone(),
            pane: self.pane,
        };
        if self.nav.get(self.nav_index) == Some(&entry) {
            return;
        }
        self.nav.truncate(self.nav_index + 1);
        self.nav.push(entry);
        if self.nav.len() > NAV_DEPTH {
            self.nav.remove(0);
        }
        self.nav_index = self.nav.len() - 1;
    }

    /// Whether back has somewhere to go.
    ///
    /// @returns true when an earlier entry exists
    pub fn can_go_back(&self) -> bool {
        self.nav_index > 0
    }

    /// Whether forward has somewhere to go.
    ///
    /// @returns true when a later entry exists
    pub fn can_go_forward(&self) -> bool {
        self.nav_index + 1 < self.nav.len()
    }

    /// Moves through the navigation history by one step.
    ///
    /// @param forward true for forward, false for back
    pub fn navigate(&mut self, forward: bool) {
        let target = if forward {
            self.nav_index + 1
        } else {
            match self.nav_index.checked_sub(1) {
                Some(index) => index,
                None => return,
            }
        };
        let Some(entry) = self.nav.get(target).cloned() else {
            return;
        };
        self.nav_index = target;
        // Opening a task, or starting a new one, records its own entry; the
        // step taken here must stand, so the history is put back after it.
        let (nav, index) = (self.nav.clone(), target);
        match &entry.task {
            Some(id) if self.current_task.as_deref() != Some(id.as_str()) => {
                self.open_task(id);
            }
            None if self.current_task.is_some() => self.new_task(),
            _ => {}
        }
        self.nav = nav;
        self.nav_index = index;
        self.pane = entry.pane;
        self.spot = SidebarSpot::Task;
    }

    /// Says something in the status bar for a few seconds.
    ///
    /// @param message what to say
    pub fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
    }

    /// The toast, when it is still fresh.
    ///
    /// @returns the message and its age
    pub fn current_toast(&self) -> Option<(&str, Duration)> {
        self.toast
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(6))
            .map(|(message, at)| (message.as_str(), at.elapsed()))
    }

    /// Persists this app's settings document.
    pub fn save_config(&mut self) {
        self.config.tree_open = self.tree_open;
        self.config.agent_panel_open = self.agent_open;
        self.config.sidebar_open = self.sidebar_open;
        if let Err(err) = self.config.save(&self.root) {
            log::warn!("config: {err}");
        }
    }

    /// Starts a fresh task: an empty transcript, and a new engine session.
    ///
    /// The engine process is kept — a session is cheap where a process is not —
    /// and the meter starts over with it, because the budget it reports is the
    /// session's, not the process's.
    pub fn new_task(&mut self) {
        self.leave_task();
        self.current_task = None;
        self.task_created = None;
        self.task_title = None;
        self.task_dirty = false;
        let next = self.agent.conversation.id.0 + 1;
        let mut conversation = Conversation::new(ConversationId(next));
        conversation.engine_name = self.agent.conversation.engine_name.clone();
        conversation.engine_version = self.agent.conversation.engine_version.clone();
        self.agent.conversation = conversation;
        self.agent.pending.clear();
        self.agent.input.clear();
        self.agent.pending_input = None;
        self.agent.last_prompt = None;
        self.agent.applied_model = None;
        self.agent.attached_block = None;
        self.reveal_call = None;
        self.pane = Pane::Conversation;
        self.spot = SidebarSpot::Task;
        self.last_activity = chrono::Local::now();
        // The engine answers with SessionReady; nothing needs saying before then.
        self.open_session();
        self.record_nav();
    }

    /// Opens the folder picker on the current workspace.
    pub fn open_folder_dialog(&mut self) {
        self.dialog = Some(Dialog::Folder(Box::new(FolderDraft::new(
            self.root.clone(),
        ))));
    }

    /// Opens a different folder: every subsystem is rebuilt for the new root.
    ///
    /// The engine is dropped rather than reused. Its session was opened on the
    /// old workspace, and the launcher read that workspace's route document when
    /// the process started; the next start is a new process whose
    /// `HARNESS_WORKSPACE` names the folder being opened.
    ///
    /// What carries over is the model route, when the new folder has none of its
    /// own: a folder switch is not a request to reconfigure the model, and a
    /// workspace with no route document would otherwise drop to the placeholder
    /// and fail every turn.
    ///
    /// @param root the folder to open
    pub fn switch_workspace(&mut self, root: PathBuf) {
        if !root.is_dir() {
            self.toast(format!("{} is not a directory", root.display()));
            return;
        }
        if root == self.root {
            self.toast("that folder is already open");
            return;
        }
        // The old workspace keeps the layout it was left in, and its task.
        self.save_config();
        self.leave_task();
        let previous = std::mem::replace(&mut self.root, root.clone());
        let applied = harness_core::config::load_dotenv_over(&root);
        if !applied.is_empty() {
            log::info!(
                "env: {} variable(s) from {}/.env",
                applied.len(),
                root.display()
            );
        }

        self.config = HarnessConfig::load(&root);
        self.tree_open = self.config.tree_open;
        self.agent_open = self.config.agent_panel_open;
        self.paths = EnginePaths::resolve(&root, self.dsh_home.clone());
        if let Err(err) = &self.paths {
            log::warn!("engine: {err}");
        }

        let models_path = HarnessConfig::models_path_for(&root);
        let carried = self.models.active.is_some().then(|| self.models.clone());
        self.models = match ModelsDocument::load(&models_path) {
            Ok(models) => models,
            Err(err) => {
                log::error!("models: {err}");
                self.toast(format!("reading {} failed: {err}", models_path.display()));
                ModelsDocument::default()
            }
        };
        let mut carried_note = None;
        if self.models.active.is_none() {
            if let Some(document) = carried {
                self.models = document;
                match self.models.save(&models_path) {
                    Ok(()) => {
                        carried_note = Some(format!(
                            "the model route carried over into {}",
                            models_path.display()
                        ))
                    }
                    Err(err) => log::warn!("models: {err}"),
                }
            }
        }
        self.active_route = self
            .models
            .active
            .as_ref()
            .map(|route| format!("{}/{}", route.provider, route.model));
        self.prices = Prices::load(&HarnessConfig::prices_path_for(&root)).unwrap_or_default();

        self.git = GitRepo::discover(&root);
        self.tree = TreeState::new();
        self.palette_files = None;
        self.editor = EditorState::new();
        self.git_view = GitView::default();
        self.search.query.clear();
        self.search.query_of_outcome.clear();
        self.search.outcome = None;
        self.search.pending = false;
        self.search.generation += 1;

        // The old shells ran in the old folder; their terminals go with it.
        self.shells.clear();
        self.active_shell = 0;

        self.agent.client = None; // dropping the client ends the process
        self.agent.conversation =
            Conversation::new(ConversationId(self.agent.conversation.id.0 + 1));
        self.agent.pending.clear();
        self.agent.input.clear();
        self.agent.pending_input = None;
        self.agent.last_prompt = None;
        self.agent.starting = false;
        self.agent.start_error = None;
        self.agent.requested_route = self.active_route.clone();
        self.agent.applied_model = None;
        self.agent.attached_block = None;
        self.agent.live_sessions.clear();
        self.reveal_call = None;
        self.current_task = None;
        self.task_created = None;
        self.task_title = None;
        self.task_dirty = false;
        self.tasks = TaskStore::for_workspace(&root).list();
        self.nav = vec![NavEntry {
            task: None,
            pane: Pane::Conversation,
        }];
        self.nav_index = 0;
        self.sidebar_open = self.config.sidebar_open;

        self.workflows = WorkflowStore::load(&HarnessConfig::workflows_dir_for(&root));
        self.refresh_notes();
        self.pane = Pane::Conversation;
        self.spot = SidebarSpot::Task;
        self.dialog = None;
        self.palette = None;
        self.last_activity = chrono::Local::now();
        self.new_shell();

        let mut notice = format!("working in {}", root.display());
        if let Some(note) = &carried_note {
            notice.push_str(&format!(" · {note}"));
        }
        self.agent.conversation.notice(NoticeLevel::Info, notice);
        log::info!(
            "workspace: switched {} → {}",
            previous.display(),
            root.display()
        );
        self.ensure_engine();
        self.toast(format!("opened {}", root.display()));
    }

    /// The task's name: the first thing asked, or what the workspace is.
    ///
    /// @returns the title shown on the conversation's title row
    pub fn session_title(&self) -> String {
        if let Some(title) = &self.task_title {
            return title.clone();
        }
        for item in &self.agent.conversation.items {
            if let harness_core::agent::Item::User { text, .. } = item {
                let line = text
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("")
                    .trim();
                if !line.is_empty() {
                    return TaskStore::title_for(line);
                }
            }
        }
        let name = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            "New task".to_string()
        } else {
            format!("Work in {name}")
        }
    }

    /// The delegations the engine has started this session, newest last.
    ///
    /// @returns the rows the panel lists
    pub fn delegations(&self) -> Vec<Delegation> {
        self.agent
            .conversation
            .items
            .iter()
            .flat_map(|item| match item {
                harness_core::agent::Item::Tool(card) if card.is_delegation() => {
                    // One run_wave call is several agents: one row per part, with
                    // the part's own outcome once the wave reports it.
                    let parts = wave_parts(card);
                    if !parts.is_empty() {
                        return parts;
                    }
                    // The committee names its subject differently to a subagent;
                    // both read as "what was delegated" in the panel.
                    let task = card
                        .input_text(&["description", "decision", "task", "summary", "prompt"])
                        .unwrap_or_else(|| "delegated work".to_string());
                    vec![Delegation {
                        agent: capitalize(card.title.trim()),
                        task: task.chars().take(96).collect(),
                        call_id: card.id.clone(),
                        done: card.status.done(),
                        failed: card.status == harness_core::agent::ToolStatus::Failed,
                    }]
                }
                _ => Vec::new(),
            })
            .collect()
    }

    /// The commands the panel lists, newest last: the engine's own tool calls
    /// and the user's shells.
    ///
    /// Every shell appears, always: a shell sitting at its prompt is part of what
    /// is running in this workspace, and a panel that only ever listed activity
    /// would go blank exactly when someone looked at it to check.
    ///
    /// @returns the rows the panel lists
    pub fn processes(&self) -> Vec<Process> {
        let mut rows: Vec<Process> = self
            .agent
            .conversation
            .items
            .iter()
            .filter_map(|item| match item {
                harness_core::agent::Item::Tool(card) if card.is_command() => {
                    let command = card
                        .input_text(&["command", "cmd", "script", "description", "title"])
                        .unwrap_or_else(|| card.title.clone());
                    Some(Process {
                        command: first_line(&command),
                        state: card.status.label().to_string(),
                        running: !card.status.done(),
                        source: ProcessSource::Tool(card.id.clone()),
                    })
                }
                _ => None,
            })
            .collect();
        for (index, shell) in self.shells.iter().enumerate() {
            let Some(terminal) = shell.terminal.as_ref() else {
                rows.push(Process {
                    command: shell.name.clone(),
                    state: "not started".to_string(),
                    running: false,
                    source: ProcessSource::Shell(index),
                });
                continue;
            };
            if terminal.blocks_running() {
                let command = terminal
                    .current_command()
                    .map(|command| first_line(&command))
                    .unwrap_or_else(|| "a command is running".to_string());
                rows.push(Process {
                    command,
                    state: "running".to_string(),
                    running: true,
                    source: ProcessSource::Shell(index),
                });
                continue;
            }
            let (command, state) = match terminal.current_command() {
                Some(command) => (first_line(&command), "finished"),
                None if terminal.is_alive() => (shell.name.clone(), "at the prompt"),
                None => (shell.name.clone(), "exited"),
            };
            rows.push(Process {
                command,
                state: state.to_string(),
                running: false,
                source: ProcessSource::Shell(index),
            });
        }
        // Running first, then the rest in the order they are known, so the panel
        // leads with what is happening rather than with history.
        rows.sort_by_key(|row| !row.running);
        rows
    }

    /// What the session is doing right now.
    ///
    /// Derived from the transcript rather than announced by the engine, so it is
    /// always there while a prompt runs — which is the whole point: a turn that
    /// is thinking looks exactly like a turn that died unless something says
    /// otherwise.
    ///
    /// @returns the activity, or nothing when no turn is in flight
    pub fn activity(&self) -> Option<Activity> {
        let conversation = &self.agent.conversation;
        let elapsed = conversation.prompt_elapsed();
        match &conversation.state {
            RunState::Idle | RunState::Failed { .. } => None,
            RunState::AwaitingApproval { title } => Some(Activity {
                headline: "waiting for your approval".to_string(),
                detail: Some(title.clone()),
                elapsed,
            }),
            RunState::Running if conversation.session_id.is_none() => Some(Activity {
                headline: "starting the engine".to_string(),
                detail: Some("the first session takes a few seconds; the prompt is sent the moment it opens".to_string()),
                elapsed,
            }),
            RunState::Running => {
                if let Some(card) = conversation.items.iter().rev().find_map(|item| match item {
                    harness_core::agent::Item::Tool(card) if !card.status.done() => Some(card),
                    _ => None,
                }) {
                    let title = card.title.trim();
                    let headline = if card.is_command() {
                        "running a command".to_string()
                    } else if card.is_delegation() {
                        format!("{} is working", capitalize(title))
                    } else if card.is_todo_write() {
                        "updating the plan".to_string()
                    } else {
                        format!("{} {}", card.status.label(), title)
                    };
                    let detail = card
                        .input_text(&[
                            "command",
                            "cmd",
                            "script",
                            "file_path",
                            "path",
                            "description",
                            "prompt",
                        ])
                        .map(|text| one_line(&text, 120));
                    return Some(Activity {
                        headline,
                        detail,
                        elapsed,
                    });
                }
                // No call is open, so the model itself is the work: say which
                // half of it, and show the last words it produced.
                match conversation.items.last() {
                    Some(harness_core::agent::Item::Assistant { text, thinking, .. })
                        if !text.trim().is_empty() || !thinking.trim().is_empty() =>
                    {
                        let streaming = !text.trim().is_empty();
                        let source = if streaming {
                            text.as_str()
                        } else {
                            thinking.as_str()
                        };
                        Some(Activity {
                            headline: if streaming {
                                "writing the answer".to_string()
                            } else {
                                "thinking".to_string()
                            },
                            detail: Some(prose_tail(source, 120)),
                            elapsed,
                        })
                    }
                    _ => Some(Activity {
                        headline: "waiting for the model".to_string(),
                        detail: None,
                        elapsed,
                    }),
                }
            }
        }
    }

    /// The panel's notice: when the session last moved, and what it last said.
    ///
    /// @returns the timestamp line and the body, or nothing before a session
    pub fn notice(&self) -> Option<(String, String)> {
        let stamp = format!("Updated at {}", self.last_activity.format("%H:%M"));
        let body = self
            .agent
            .conversation
            .items
            .iter()
            .rev()
            .find_map(|item| match item {
                harness_core::agent::Item::Assistant { text, .. } if !text.trim().is_empty() => {
                    Some(plain_summary(text, 320))
                }
                _ => None,
            })
            .or_else(|| {
                self.agent
                    .conversation
                    .items
                    .iter()
                    .rev()
                    .find_map(|item| match item {
                        harness_core::agent::Item::Notice { text, .. } => Some(text.clone()),
                        _ => None,
                    })
            })
            // Before the first answer, the panel says what it was asked —
            // never "nothing has been asked yet" while the agent is working.
            .or_else(|| {
                self.agent.conversation.first_prompt().map(|asked| {
                    format!("Working on: {}", plain_summary(asked, 280))
                })
            });
        body.map(|body| (stamp, body))
    }

    /// How full the context window is, as the footer prints it.
    ///
    /// @returns a percentage, or nothing before the engine reports usage
    pub fn context_percent(&self) -> Option<String> {
        let meter = &self.agent.conversation.meter;
        let fraction = meter.context_fraction();
        if meter.size == 0 || meter.used == 0 {
            return None;
        }
        Some(format!("{:.0}%", (fraction * 100.0).clamp(0.0, 100.0)))
    }

    /// Opens a tool call in the conversation, expanded.
    ///
    /// @param call_id the tool call to show
    pub fn reveal(&mut self, call_id: impl Into<String>) {
        self.reveal_call = Some(call_id.into());
        self.pane = Pane::Conversation;
        self.spot = SidebarSpot::Task;
    }

    /// Switches to a view from the sidebar.
    ///
    /// @param pane the view to show
    /// @param spot the row that asked for it
    pub fn go(&mut self, pane: Pane, spot: SidebarSpot) {
        self.pane = pane;
        self.spot = spot;
        self.record_nav();
    }

    /// The engine layout, when it was found.
    ///
    /// @returns the paths or the reason they are missing
    pub fn paths(&self) -> Result<&EnginePaths, &str> {
        self.paths.as_ref().map_err(String::as_str)
    }

    /// Starts a shell in the workspace.
    ///
    /// @returns the index of the new shell
    pub fn new_shell(&mut self) -> usize {
        let id = TerminalId(self.next_terminal);
        self.next_terminal += 1;
        let name = format!("shell {}", self.shells.len() + 1);
        self.shells.push(Shell::new(id, name));
        let index = self.shells.len() - 1;
        self.start_shell(index);
        self.active_shell = index;
        index
    }

    /// Starts the PTY for a shell that has none.
    ///
    /// @param index which shell
    pub fn start_shell(&mut self, index: usize) {
        let Some(shell) = self.shells.get(index) else {
            return;
        };
        if shell.terminal.is_some() {
            return;
        }
        let id = shell.id;
        let name = shell.name.clone();
        let shell_path = std::env::var("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/bin/sh"));
        let shell_name = shell_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("sh")
            .to_string();

        // Shell integration is what turns a byte stream into blocks. It is
        // written per shell under `.harness/shell`, because it has to be a file
        // the user's own rc can source.
        let dir = self.root.join(".harness").join("shell");
        let integration = write_shell_integration(&dir, &shell_name).unwrap_or_else(|err| {
            log::warn!("shell integration: {err}");
            None
        });
        let (env, args) = match &integration {
            Some(integration) => (integration.env.clone(), integration.args.clone()),
            None => (Vec::new(), Vec::new()),
        };
        let options = TerminalOptions {
            shell: shell_path,
            args,
            cwd: self.root.clone(),
            size: ScreenSize {
                cols: 100,
                rows: 30,
            },
            env,
            scrollback: SCROLLBACK,
        };
        match Terminal::spawn(id, options, self.sink.clone()) {
            Ok(terminal) => {
                log::info!(
                    "terminal: {name} started ({})",
                    if integration.is_some() {
                        "integrated"
                    } else {
                        "no integration"
                    }
                );
                if let Some(shell) = self.shells.get_mut(index) {
                    shell.terminal = Some(terminal);
                    shell.error = None;
                }
            }
            Err(err) => {
                log::error!("terminal: {name} failed: {err}");
                if let Some(shell) = self.shells.get_mut(index) {
                    shell.error = Some(err);
                }
            }
        }
    }

    /// The shell the terminal pane is showing, mutably.
    ///
    /// @returns the shell, when there is one
    pub fn shell_mut(&mut self) -> Option<&mut Shell> {
        self.shells.get_mut(self.active_shell)
    }

    /// Re-reads the block list of a shell, when a command has finished since.
    ///
    /// The blocks tab is the only reader, and it reads on demand: cloning every
    /// block's output on every frame would cost more than the pane is worth.
    ///
    /// @param index which shell
    pub fn refresh_blocks(&mut self, index: usize) {
        let Some(shell) = self.shells.get_mut(index) else {
            return;
        };
        if !shell.blocks_dirty {
            return;
        }
        if let Some(terminal) = shell.terminal.as_ref() {
            let mut blocks = terminal.recent_blocks(BLOCK_LIST);
            blocks.reverse(); // oldest first, the way a terminal reads
            shell.blocks = blocks;
            if shell.selected_block.is_none() {
                shell.selected_block = shell.blocks.len().checked_sub(1);
            }
        }
        shell.blocks_dirty = false;
    }

    /// Attaches a finished command's output to the agent's prompt.
    ///
    /// This is the harness's answer to copying a stack trace into a chat: the
    /// block is already here, so it is carried verbatim — clipped, because
    /// sending a megabyte of build log to a model is not a favour to anyone.
    ///
    /// @param index which block
    pub fn attach_block(&mut self, index: usize) {
        let Some(shell) = self.shells.get(self.active_shell) else {
            return;
        };
        let Some(block) = shell.blocks.get(index) else {
            return;
        };
        let exit = block
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "running".to_string());
        let label = format!("{} · exit {exit}", block.command);
        let output = clip(&block.output, BLOCK_PROMPT_LIMIT);
        let note = if block.output.len() > output.len() {
            "\n… (output clipped)\n"
        } else {
            ""
        };
        self.agent.input = format!(
            "This command's output needs attention:\n\n```console\n$ {}\n{output}{note}```\n\n",
            block.command
        );
        self.agent.attached_block = Some(label);
        self.agent_open = true;
        self.focus = Some(FocusRequest::Agent);
        self.save_config();
    }

    /// Types a command into the active shell and runs it.
    ///
    /// @param command the command line
    pub fn run_command(&mut self, command: &str) {
        if let Some(shell) = self.shell_mut() {
            if let Some(terminal) = shell.terminal.as_ref() {
                terminal.run(command);
                shell.tab = ShellTab::Screen;
                shell.follow = true;
                self.pane = Pane::Terminal;
            } else {
                self.toast("this shell is not running");
            }
        }
    }

    /// Opens a file in the editor.
    ///
    /// @param path file to open
    pub fn open_file(&mut self, path: &Path) {
        match fsops::read_text(&self.root, path, fsops::TEXT_LOAD_LIMIT) {
            Ok(FileText {
                relative,
                text,
                bytes,
                truncated,
                binary,
            }) => {
                log::info!("editor: opened {relative}");
                self.editor.buffer = Some(Buffer {
                    path: path.to_path_buf(),
                    language: language_of(path),
                    relative,
                    saved: text.clone(),
                    text,
                    bytes,
                    truncated,
                    binary,
                    editing: false,
                    scroll_line: None,
                });
                self.editor.error = None;
                self.pane = Pane::Editor;
            }
            Err(err) => {
                self.editor.error = Some(err.clone());
                self.toast(err);
            }
        }
    }

    /// Writes the open buffer back to disk.
    ///
    /// @returns nothing; the pane shows any error
    pub fn save_buffer(&mut self) {
        let Some(buffer) = self.editor.buffer.as_mut() else {
            return;
        };
        let outcome = fsops::write_text(&buffer.path, &buffer.text);
        if outcome.is_ok() {
            buffer.saved = buffer.text.clone();
            buffer.bytes = buffer.text.len() as u64;
        }
        let relative = buffer.relative.clone();
        // The buffer borrow ends here, so the state can record what happened.
        match outcome {
            Ok(()) => {
                self.editor.error = None;
                self.toast(format!("saved {relative}"));
                self.tree.invalidate();
                self.refresh_git();
            }
            Err(err) => {
                self.editor.error = Some(err.clone());
                self.toast(err);
            }
        }
    }

    /// Re-reads repository state.
    pub fn refresh_git(&mut self) {
        let Some(git) = self.git.as_ref() else { return };
        match git.status() {
            Ok(status) => {
                self.git_view.status = status;
                self.git_view.error = None;
            }
            Err(err) => self.git_view.error = Some(err),
        }
        self.git_view.branches = git.branches().unwrap_or_default();
        self.git_view.log = git.log(50).unwrap_or_default();
        self.git_view.loaded = true;
        if let Some(path) = self.git_view.selected.clone() {
            self.load_diff(&path, self.git_view.staged);
        }
    }

    /// Reads the patch for one path into the pane.
    ///
    /// @param path path relative to the repository root
    /// @param staged whether to read the staged side
    pub fn load_diff(&mut self, path: &str, staged: bool) {
        let Some(git) = self.git.as_ref() else { return };
        match git.diff(path, staged) {
            Ok(patch) => {
                // A generated file, a lockfile, or a vendored tree produces a
                // patch nobody reads line by line; the collapsed view is the
                // honest default for one, and the pane can still show it whole.
                self.git_view.compact = patch.lines().count() > COMPACT_DIFF_LINES;
                self.git_view.diff = Some((path.to_string(), patch));
            }
            Err(err) => {
                self.git_view.diff = None;
                self.git_view.error = Some(err);
            }
        }
    }

    /// Stages or unstages one path and re-reads the status.
    ///
    /// @param path path relative to the repository root
    /// @param stage true to stage, false to unstage
    pub fn set_staged(&mut self, path: &str, stage: bool) {
        let Some(git) = self.git.as_ref() else { return };
        let result = if stage {
            git.stage(path)
        } else {
            git.unstage(path)
        };
        match result {
            Ok(()) => {
                self.git_view.selected = Some(path.to_string());
                self.git_view.staged = stage;
                self.refresh_git();
            }
            Err(err) => {
                self.git_view.error = Some(err.clone());
                self.toast(err);
            }
        }
    }

    /// Commits what is staged.
    pub fn commit(&mut self) {
        let message = self.git_view.message.trim().to_string();
        if message.is_empty() {
            self.toast("a commit needs a message");
            return;
        }
        let Some(git) = self.git.as_ref() else { return };
        match git.commit(&message) {
            Ok(short) => {
                self.git_view.message.clear();
                self.toast(format!("committed {short}"));
                self.refresh_git();
            }
            Err(err) => {
                self.git_view.error = Some(err.clone());
                self.toast(err);
            }
        }
    }

    /// Lists the notes under `.harness/notes`.
    pub fn refresh_notes(&mut self) {
        let dir = self.notes_dir();
        let mut notes = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                    notes.push(path);
                }
            }
        }
        notes.sort();
        self.notes = notes;
    }

    /// @returns the directory notes live in
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join(".harness").join("notes")
    }

    /// Writes a note, creating the directory.
    ///
    /// @param name file name, `.md` appended when missing
    /// @param text the note's contents
    /// @returns the path written
    /// @throws `Err` when the file cannot be written
    pub fn save_note(&self, name: &str, text: &str) -> Result<PathBuf, String> {
        let file = if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        let path = self.notes_dir().join(file);
        fsops::write_text(&path, text)?;
        Ok(path)
    }

    /// Removes a note, so a rename does not leave the old file behind.
    ///
    /// @param name file name, with or without `.md`
    pub fn remove_note(&self, name: &str) {
        let file = if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        if let Err(err) = std::fs::remove_file(self.notes_dir().join(file)) {
            if err.kind() != std::io::ErrorKind::NotFound {
                log::warn!("notes: {err}");
            }
        }
    }

    /// Refreshes the palette's file list.
    pub fn refresh_palette_files(&mut self) {
        self.palette_files = Some(fsops::list_files(&self.root, PALETTE_FILES, false));
    }

    /// Runs the current search pattern on its own thread.
    ///
    /// The walk covers the workspace and takes as long as it takes; it never
    /// happens inside a frame. Each request carries a generation, so a result
    /// for a query the user has already typed over arrives and is dropped.
    pub fn run_search(&mut self) {
        self.search.generation += 1;
        let generation = self.search.generation;
        let query = self.search.query.trim().to_string();
        if query.is_empty() {
            self.search.outcome = None;
            self.search.query_of_outcome.clear();
            self.search.pending = false;
            return;
        }
        self.search.pending = true;
        let root = self.root.clone();
        let options = self.search.options;
        let sink = self.search.sink.clone();
        let spawned = std::thread::Builder::new()
            .name("harness-search".into())
            .spawn(move || {
                let outcome = fsops::search(&root, &query, &options);
                let _ = sink.send((generation, query, outcome));
            });
        if let Err(err) = spawned {
            self.search.pending = false;
            log::error!("search: {err}");
        }
    }

    /// Asks for a pattern unless it is already the one being answered.
    ///
    /// @param query the pattern
    pub fn search_for(&mut self, query: &str) {
        if self.search.query == query {
            return;
        }
        self.search.query = query.to_string();
        self.run_search();
    }

    /// Recomputes the palette's hits for its mode and query.
    ///
    /// Called by the palette whenever its query changed, which is why it does no
    /// work otherwise: a palette that rebuilds an empty list every frame is a
    /// palette that wastes the frame.
    pub fn refresh_palette(&mut self) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        if !palette.dirty {
            return;
        }
        let mode = palette.mode;
        let query = palette.query.clone();
        if mode == PaletteMode::Files && self.palette_files.is_none() {
            self.refresh_palette_files();
        }
        if mode == PaletteMode::Contents {
            self.search_for(&query);
        }
        let hits = match mode {
            PaletteMode::Commands => self.command_hits(&query),
            PaletteMode::Files => self.file_hits(&query),
            PaletteMode::Contents => self.content_hits(&query),
        };
        if let Some(palette) = self.palette.as_mut() {
            palette.selection = palette.selection.min(hits.len().saturating_sub(1));
            palette.hits = hits;
            palette.dirty = false;
        }
    }

    /// The palette's command list: the app's actions, then the workspace's own
    /// workflows and notes.
    ///
    /// @param query what was typed
    /// @returns hits, best first
    fn command_hits(&self, query: &str) -> Vec<PaletteHit> {
        let mut hits = Vec::new();
        for (label, detail, action) in [
            (
                "Terminal pane",
                "⌘1 · the shell and its blocks",
                PaletteAction::Pane(Pane::Terminal),
            ),
            (
                "Editor pane",
                "⌘2 · open files and edit them",
                PaletteAction::Pane(Pane::Editor),
            ),
            (
                "Git pane",
                "⌘3 · status, diffs, commit",
                PaletteAction::Pane(Pane::Git),
            ),
            (
                "Workflows pane",
                "⌘4 · saved commands and prompts",
                PaletteAction::Pane(Pane::Workflows),
            ),
            (
                "Observability pane",
                "⌘5 · tokens, context, tools",
                PaletteAction::Pane(Pane::Observability),
            ),
            (
                "New shell",
                "⌘T · another PTY in this workspace",
                PaletteAction::NewShell,
            ),
            ("Toggle file tree", "⌘B", PaletteAction::ToggleTree),
            ("Toggle agent panel", "⌘J", PaletteAction::ToggleAgent),
            ("Save file", "⌘S", PaletteAction::SaveBuffer),
            (
                "Export transcript",
                "⌘⇧E · write the session to .harness/transcripts",
                PaletteAction::ExportTranscript,
            ),
            (
                "Search file contents",
                "⌘⇧F",
                PaletteAction::Palette(PaletteMode::Contents),
            ),
            (
                "Open folder",
                "⌘O · work in a different folder",
                PaletteAction::Dialog(DialogKind::Folder),
            ),
            (
                "Model routes",
                "add, edit, or switch the model the session runs on",
                PaletteAction::Dialog(DialogKind::Models),
            ),
            (
                "New workflow",
                "a command or prompt to save",
                PaletteAction::Dialog(DialogKind::Workflow),
            ),
            (
                "New note",
                "a markdown note in .harness/notes",
                PaletteAction::Dialog(DialogKind::Note),
            ),
            (
                "About AI Harness",
                "what this is, and what it will not do",
                PaletteAction::Dialog(DialogKind::About),
            ),
        ] {
            hits.push(PaletteHit {
                label: label.to_string(),
                detail: detail.to_string(),
                action,
            });
        }

        for workflow in self.workflows.all() {
            let kind = match workflow.kind {
                WorkflowKind::Command => "command",
                WorkflowKind::Prompt => "prompt",
            };
            let filled = workflow.render(&BTreeMap::new());
            let detail = if filled.missing.is_empty() {
                format!("{kind} · {}", one_line(&filled.text, 68))
            } else {
                format!("{kind} · needs {}", filled.missing.join(", "))
            };
            hits.push(PaletteHit {
                label: workflow.name.clone(),
                detail,
                action: PaletteAction::Workflow(workflow.id.clone()),
            });
        }

        for note in &self.notes {
            let name = note
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("note")
                .to_string();
            hits.push(PaletteHit {
                label: name,
                detail: format!("note · {}", fsops::relative(&self.root, note)),
                action: PaletteAction::OpenFile(note.clone()),
            });
        }

        let mut ranked = rank(hits, query, COMMAND_HITS);
        // What was typed is itself actionable: run it in the shell, or hand it
        // to the agent. These sit above everything else, because a person who
        // typed a command in the palette almost always meant to run it.
        let typed = query.trim();
        if !typed.is_empty() {
            ranked.insert(
                0,
                PaletteHit {
                    label: typed.to_string(),
                    detail: "hand to the agent".to_string(),
                    action: PaletteAction::Prompt(typed.to_string()),
                },
            );
            ranked.insert(
                0,
                PaletteHit {
                    label: typed.to_string(),
                    detail: "run in the terminal".to_string(),
                    action: PaletteAction::RunCommand(typed.to_string()),
                },
            );
        }
        ranked
    }

    /// The palette's file list, fuzzy-matched.
    ///
    /// @param query what was typed
    /// @returns hits, best first
    fn file_hits(&self, query: &str) -> Vec<PaletteHit> {
        let files = self.palette_files.as_deref().unwrap_or_default();
        let mut scored: Vec<(i32, PaletteHit)> = Vec::new();
        for path in files {
            let Some(score) = fsops::fuzzy_score(query, path) else {
                continue;
            };
            let name = path.rsplit('/').next().unwrap_or(path);
            scored.push((
                score,
                PaletteHit {
                    label: name.to_string(),
                    detail: path.clone(),
                    action: PaletteAction::OpenFile(self.root.join(path)),
                },
            ));
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.detail.len().cmp(&b.1.detail.len()))
        });
        scored.truncate(FILE_HITS);
        scored.into_iter().map(|(_, hit)| hit).collect()
    }

    /// The palette's content hits, from the threaded search.
    ///
    /// @param query what was typed
    /// @returns hits, best first
    fn content_hits(&self, query: &str) -> Vec<PaletteHit> {
        let Some(outcome) = self.search.outcome.as_ref() else {
            return Vec::new();
        };
        if self.search.query_of_outcome.trim() != query.trim() {
            return Vec::new();
        }
        outcome
            .matches
            .iter()
            .map(|found| PaletteHit {
                label: found.text.trim().to_string(),
                detail: format!("{}:{}", found.path, found.line),
                action: PaletteAction::OpenAt(self.root.join(&found.path), found.line),
            })
            .collect()
    }

    /// Starts the engine and opens a session, when a route is configured.
    ///
    /// Called when the agent panel first needs an engine; a workspace with no
    /// model route shows the empty state instead, because there is nothing to
    /// start the engine on.
    pub fn ensure_engine(&mut self) {
        if self.agent.client.is_some() || self.agent.starting {
            return;
        }
        // No `.harness/models.json` route is not "no model": the composition
        // serves its default route (AI_MODEL, or OpenRouter's DeepSeek), so the
        // engine starts either way and the route it names is what runs.
        let paths = match self.paths() {
            Ok(paths) => paths.clone(),
            Err(err) => {
                self.agent.start_error = Some(err.to_string());
                return;
            }
        };
        self.agent.starting = true;
        match AcpClient::spawn(&paths, self.agent.sender.clone(), &self.root) {
            Ok(client) => {
                log::info!(
                    "engine: started via {} ({})",
                    paths.launcher.display(),
                    paths.dsh_home.display()
                );
                client.initialize();
                self.agent.requested_route = self.active_route.clone();
                self.agent.client = Some(client);
                self.agent.start_error = None;
            }
            Err(err) => {
                log::error!("engine: {err}");
                self.agent.start_error = Some(err);
            }
        }
        self.agent.starting = false;
    }

    /// Restarts the engine, so a changed route registry takes effect.
    ///
    /// The composition reads the routes at launch, so a new or edited route
    /// needs a fresh process; the transcript stays, because it is the app's.
    pub fn restart_engine(&mut self) {
        self.save_task();
        // Keep the engine's durable context as well as the visible transcript.
        if let Some(session) = self.agent.conversation.session_id.take() {
            self.agent.resume = Some(session);
        }
        if self.agent.conversation.state.busy() {
            self.agent
                .conversation
                .fail("the turn was interrupted by a model route change");
        }
        self.agent.live_sessions.clear();
        self.agent.pending.clear();
        self.agent.applied_model = None;
        // Retired readers can still emit events while their process shuts down.
        // A new channel prevents those events from corrupting the new session.
        let (sender, events) = std::sync::mpsc::channel();
        self.agent.sender = sender;
        self.agent.events = events;
        if self.agent.client.is_some() {
            self.agent.client = None;
            self.agent.conversation.notice(
                NoticeLevel::Info,
                "engine restarted for the new model route",
            );
            self.agent.conversation.session_id = None;
        }
        self.agent.requested_route = self.active_route.clone();
        self.ensure_engine();
    }

    /// Sends a prompt to the agent.
    ///
    /// @param text the user's message
    pub fn send_prompt(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(message) = missing_credential_message(
            self.models.active.as_ref(),
            &self.models.providers,
            |name| std::env::var(name).ok(),
        ) {
            // The message is shown like any other send, so the transcript
            // reads "you asked X — here is why it could not run", and "try
            // again" resends it once a key is in place.
            self.agent.conversation.begin_prompt(text);
            self.agent.conversation.fail(message);
            self.agent.last_prompt = Some(text.to_string());
            self.agent.input.clear();
            self.scroll_to_bottom = true;
            return;
        }
        // The prompt the user just typed is the reason to be at the bottom —
        // `stick_to_bottom` only keeps following the newest line while the
        // reader was already there, so it does nothing when the reader had
        // scrolled up even slightly, and the new message then lands off
        // screen above the fold instead of in view. Sending a prompt is an
        // unconditional reason to jump, the same as the "jump to latest"
        // button.
        self.scroll_to_bottom = true;
        self.ensure_engine();
        let route_ready = self.apply_requested_model();
        let Some(client) = self.agent.client.as_ref() else {
            let reason = self
                .agent
                .start_error
                .clone()
                .unwrap_or_else(|| "the engine could not be started".to_string());
            self.agent.conversation.fail(reason);
            return;
        };
        let Some(session) = self.agent.conversation.session_id.clone().filter(|_| route_ready) else {
            // The session is opened in the background. The prompt is on screen
            // and counted as asked right away; it reaches the engine the moment
            // the session is ready (see SessionReady in the pump).
            if self.current_task.is_none() {
                self.current_task = Some(TaskStore::new_id());
                self.task_created = Some(chrono::Local::now().to_rfc3339());
            }
            self.agent.conversation.begin_prompt(text);
            self.agent.pending_input = Some(text.to_string());
            self.agent.last_prompt = Some(text.to_string());
            self.agent.input.clear();
            self.scroll_to_bottom = true;
            self.pane = Pane::Conversation;
            self.save_task();
            return;
        };
        if self.current_task.is_none() {
            self.current_task = Some(TaskStore::new_id());
            self.task_created = Some(chrono::Local::now().to_rfc3339());
        }
        self.agent.conversation.begin_prompt(text);
        client.prompt(&session, text);
        self.task_dirty = true;
        self.agent.last_prompt = Some(text.to_string());
        self.agent.input.clear();
        self.last_activity = chrono::Local::now();
        self.spot = SidebarSpot::Task;
        self.pane = Pane::Conversation;
        self.scroll_to_bottom = true;
        // The task is in the history from its first prompt, not its first answer.
        self.save_task();
        self.record_nav();
    }

    /// Sends the last prompt again, after a turn failed.
    ///
    /// A provider that rate-limits or drops a connection fails the turn without
    /// touching the conversation, so the honest repair is to ask again rather
    /// than to make the user retype what they already wrote.
    pub fn retry_last_prompt(&mut self) {
        let Some(text) = self.agent.last_prompt.clone() else {
            self.toast("nothing to retry yet");
            return;
        };
        self.agent
            .conversation
            .notice(NoticeLevel::Info, "asking again");
        self.send_prompt(&text);
    }

    /// Answers a permission ask.
    ///
    /// @param index which ask
    /// @param option_id the chosen option, or `None` to cancel
    pub fn answer_permission(&mut self, index: usize, option_id: Option<String>) {
        if index >= self.agent.pending.len() {
            return;
        }
        let ask = self.agent.pending.remove(index);
        let label = option_id
            .as_ref()
            .and_then(|id| ask.options.iter().find(|option| &option.option_id == id))
            .map(|option| option.name.clone());
        if let Some(client) = self.agent.client.as_ref() {
            client.respond_permission(&ask.rpc_id, option_id.as_deref());
        }
        let decision = match label {
            Some(label) => label,
            None => "rejected".to_string(),
        };
        self.agent
            .conversation
            .record_decision(&ask.title, Some(&decision));
    }

    /// Writes the transcript next to the workspace.
    ///
    /// @returns the path written, or the error
    pub fn export_transcript(&mut self) -> Result<PathBuf, String> {
        let dir = self.root.join(".harness").join("transcripts");
        std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("session-{stamp}.md"));
        let text = self.agent.conversation.transcript_text();
        let header = format!(
            "# AI Harness transcript\n\n- workspace: {}\n- engine: {} {}\n- model: {}\n- tokens: {} used, {} context\n\n",
            self.root.display(),
            self.agent.conversation.engine_name,
            self.agent.conversation.engine_version,
            self.agent.applied_model.clone().unwrap_or_else(|| "unset".to_string()),
            self.agent.conversation.meter.used,
            self.agent.conversation.meter.size,
        );
        fsops::write_text(&path, &format!("{header}{text}"))?;
        Ok(path)
    }

    /// Applies a palette action.
    ///
    /// @param action what was chosen
    pub fn apply(&mut self, action: &PaletteAction) {
        match action {
            PaletteAction::OpenFile(path) => self.open_file(path),
            PaletteAction::OpenAt(path, line) => {
                self.open_file(path);
                if let Some(buffer) = self.editor.buffer.as_ref() {
                    let offset = buffer
                        .text
                        .lines()
                        .take(line.saturating_sub(1) as usize)
                        .map(|line| line.len() + 1)
                        .sum();
                    self.editor.goto = Some(offset);
                }
            }
            PaletteAction::Pane(pane) => self.pane = *pane,
            PaletteAction::NewShell => {
                self.new_shell();
                self.pane = Pane::Terminal;
            }
            PaletteAction::RunCommand(command) => self.run_command(command),
            PaletteAction::Prompt(prompt) => {
                self.agent.input = prompt.clone();
                self.agent_open = true;
                self.focus = Some(FocusRequest::Agent);
                self.save_config();
            }
            PaletteAction::Workflow(id) => {
                if let Some(workflow) = self.workflows.get(id) {
                    self.dialog = Some(Dialog::Workflow(Box::new(workflow_draft(
                        workflow.clone(),
                        false,
                    ))));
                }
            }
            PaletteAction::Palette(mode) => {
                let mut palette = Palette::new(*mode);
                palette.dirty = true;
                self.palette = Some(palette);
                self.focus = Some(FocusRequest::Palette);
            }
            PaletteAction::Dialog(kind) => match kind {
                DialogKind::Models => self.dialog = Some(Dialog::Models(self.model_draft())),
                DialogKind::Workflow => {
                    let workflow = Workflow::prompt("new-workflow", "New workflow", "");
                    self.dialog = Some(Dialog::Workflow(Box::new(workflow_draft(workflow, true))));
                }
                DialogKind::Note => {
                    self.dialog = Some(Dialog::Note(NoteDraft {
                        name: String::new(),
                        text: String::new(),
                        original: None,
                    }));
                }
                DialogKind::Folder => self.open_folder_dialog(),
                DialogKind::About => self.dialog = Some(Dialog::About),
            },
            PaletteAction::ToggleAgent => {
                self.agent_open = !self.agent_open;
                self.save_config();
            }
            PaletteAction::ToggleTree => {
                self.tree_open = !self.tree_open;
                self.save_config();
            }
            PaletteAction::SaveBuffer => self.save_buffer(),
            PaletteAction::ExportTranscript => match self.export_transcript() {
                Ok(path) => self.toast(format!("wrote {}", path.display())),
                Err(err) => self.toast(err),
            },
        }
    }

    /// A draft for the model editor, filled from the current document.
    ///
    /// @returns the draft
    pub fn model_draft(&self) -> ModelDraft {
        let route = self
            .models
            .active
            .as_ref()
            .map(|active| active.provider.clone())
            .unwrap_or_default();
        if route.is_empty() {
            return ModelDraft::empty();
        }
        self.model_draft_for(&route)
    }

    /// Opens the model editor on the document as it is on disk.
    ///
    /// The document is meant to be editable outside the app — by hand, or by
    /// another tool — and saving it writes the whole file. Re-reading first is
    /// what keeps this dialog from showing a route someone already fixed, or
    /// from dropping a route added while the app was running: the memory copy
    /// is only ever as good as the last read.
    pub fn open_model_dialog(&mut self) {
        match ModelsDocument::load(&HarnessConfig::models_path_for(&self.root)) {
            Ok(models) => {
                self.models = models;
                self.active_route = self
                    .models
                    .active
                    .as_ref()
                    .map(|route| format!("{}/{}", route.provider, route.model));
                self.prices =
                    Prices::load(&HarnessConfig::prices_path_for(&self.root)).unwrap_or_default();
            }
            Err(err) => {
                log::warn!("models: {err}");
                self.toast(format!(
                    "reading {} failed: {err}",
                    HarnessConfig::models_path_for(&self.root).display()
                ));
            }
        }
        self.dialog = Some(Dialog::Models(self.model_draft()));
    }

    /// A draft for one route, filled from the document.
    ///
    /// @param route the route key
    /// @returns the draft
    pub fn model_draft_for(&self, route: &str) -> ModelDraft {
        let profile = self.models.providers.get(route);
        let models = profile
            .and_then(|profile| profile.models.as_deref())
            .map(|models| {
                models
                    .iter()
                    .map(|model| {
                        // The parser on save (`save_model_draft`) reads a line
                        // positionally as `id | name | context | max`: writing
                        // only the fields that happen to be set, in order, once
                        // silently shifted a bare context_window (no name) into
                        // the name slot on the very next save, and dropped
                        // max_tokens outright (it was never written here at
                        // all) — a route saved with only a context window
                        // configured lost it, and quietly renamed itself to
                        // that number, on a no-op open-then-save. Once any of
                        // the three trailing fields is set, all three are
                        // written (blank for the ones that are not), so a
                        // field's position always matches what it means.
                        let mut line = model.id.clone();
                        if model.name.is_some() || model.context_window.is_some() || model.max_tokens.is_some() {
                            line.push_str(&format!(
                                " | {} | {} | {}",
                                model.name.as_deref().unwrap_or(""),
                                model.context_window.map(|window| window.to_string()).unwrap_or_default(),
                                model.max_tokens.map(|max| max.to_string()).unwrap_or_default(),
                            ));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let selected = self
            .models
            .active
            .as_ref()
            .filter(|active| active.provider == route)
            .map(|active| active.model.clone())
            .or_else(|| {
                profile
                    .and_then(|profile| profile.models.as_ref())
                    .and_then(|models| models.first())
                    .map(|model| model.id.clone())
            })
            .unwrap_or_default();
        let price = self.prices.get(route, &selected);
        ModelDraft {
            route: route.to_string(),
            display_name: profile
                .and_then(|profile| profile.display_name.clone())
                .unwrap_or_default(),
            base_url: profile
                .and_then(|profile| profile.base_url.clone())
                .unwrap_or_default(),
            api_key_env: profile
                .and_then(|profile| profile.api_key_env.clone())
                .unwrap_or_else(|| "AI_API_KEY".to_string()),
            // Deliberately blank: the document holds the variable's name, and
            // reading the value back out of the environment would put the
            // credential on screen for anyone walking past.
            api_key: String::new(),
            models,
            active: selected,
            price_input: price
                .map(|price| format!("{}", price.input))
                .unwrap_or_default(),
            price_output: price
                .map(|price| format!("{}", price.output))
                .unwrap_or_default(),
            existing: true,
        }
    }

    /// Makes a route and model the one the session runs on.
    ///
    /// Writing the choice is what the engine reads at startup, so this also
    /// restarts it: the composition mounts its routes when it launches, and a
    /// session that kept running on the old one would quietly ignore the click.
    ///
    /// @param route the route key
    /// @param model the model id
    pub fn activate_route(&mut self, route: &str, model: &str) {
        self.models.active = Some(harness_core::config::ActiveRoute {
            provider: route.to_string(),
            model: model.to_string(),
        });
        let path = HarnessConfig::models_path_for(&self.root);
        if let Err(err) = self.models.save(&path) {
            self.toast(format!("could not write {}: {err}", path.display()));
            return;
        }
        self.active_route = Some(format!("{route}/{model}"));
        self.toast(format!("the session runs on {route}/{model}"));
        self.restart_engine();
    }

    /// Writes the model editor's draft into the workspace documents.
    ///
    /// @param draft the edited route
    /// @returns nothing; the toast says what happened
    pub fn save_model_draft(&mut self, draft: &ModelDraft) {
        let route = draft.route.trim();
        if route.is_empty() {
            self.toast("a route needs a name");
            return;
        }
        let base_url = match harness_core::config::validate_base_url(&draft.base_url) {
            Ok(url) => url,
            Err(reason) => {
                self.toast(reason);
                return;
            }
        };
        let key_var = if draft.api_key_env.trim().is_empty() {
            "AI_API_KEY"
        } else {
            draft.api_key_env.trim()
        };
        // The credential is written before the route is: `.env` is the only
        // place a value belongs, and a refused pair must not leave a route
        // behind that names a variable the user believes they just filled in.
        let mut key_note = None;
        if !draft.api_key.trim().is_empty() {
            match harness_core::config::write_dotenv_value(
                &self.root,
                key_var,
                draft.api_key.trim(),
            ) {
                Ok(path) => {
                    // The engine child inherits this process's environment, so
                    // the restart below starts with the value already present.
                    std::env::set_var(key_var, draft.api_key.trim());
                    key_note = Some(format!("{key_var} written to {}", path.display()));
                }
                Err(err) => {
                    self.toast(format!("nothing was saved: {err}"));
                    return;
                }
            }
        }
        let models: Vec<ProviderModel> = draft
            .models
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('|').map(str::trim);
                let id = fields.next()?.to_string();
                if id.is_empty() {
                    return None;
                }
                let name = fields
                    .next()
                    .filter(|field| !field.is_empty())
                    .map(str::to_string);
                let context = fields.next().and_then(|field| field.parse::<u32>().ok());
                let max = fields.next().and_then(|field| field.parse::<u32>().ok());
                Some(ProviderModel {
                    id,
                    name,
                    context_window: context,
                    max_tokens: max,
                })
            })
            .collect();
        if models.is_empty() {
            self.toast("a route needs at least one model id");
            return;
        }
        let profile = ProviderProfile {
            display_name: Some(if draft.display_name.trim().is_empty() {
                route.to_string()
            } else {
                draft.display_name.trim().to_string()
            }),
            api: Some("openai-completions".to_string()),
            base_url: Some(base_url),
            api_key_env: Some(if draft.api_key_env.trim().is_empty() {
                "AI_API_KEY".to_string()
            } else {
                draft.api_key_env.trim().to_string()
            }),
            models: Some(models.clone()),
        };
        self.models.upsert_route(route, profile);

        // The active model: the draft's choice when it is one of the route's,
        // otherwise the first one, so saving a route always leaves something to
        // run on.
        let chosen = models
            .iter()
            .find(|model| model.id == draft.active.trim())
            .map(|model| model.id.clone())
            .unwrap_or_else(|| models[0].id.clone());
        self.models.active = Some(harness_core::config::ActiveRoute {
            provider: route.to_string(),
            model: chosen.clone(),
        });
        let path = HarnessConfig::models_path_for(&self.root);
        if let Err(err) = self.models.save(&path) {
            self.toast(format!("could not write {}: {err}", path.display()));
            return;
        }

        if let (Ok(input), Ok(output)) = (
            draft.price_input.trim().parse::<f64>(),
            draft.price_output.trim().parse::<f64>(),
        ) {
            self.prices.models.insert(
                format!("{route}/{chosen}"),
                Price {
                    input,
                    output,
                    cache_read: None,
                },
            );
            if let Err(err) = self
                .prices
                .save(&HarnessConfig::prices_path_for(&self.root))
            {
                log::warn!("prices: {err}");
            }
        }

        self.active_route = Some(format!("{route}/{chosen}"));
        self.toast(match &key_note {
            Some(note) => format!("route {route} saved and {note}"),
            None => format!("route {route} saved; the engine will start on {chosen}"),
        });
        self.dialog = None;
        // Where the key comes from is the question a failed turn raises, so the
        // notice answers it up front — including when the answer is "nowhere".
        let key_state = match std::env::var(key_var) {
            Ok(value) if value.trim() == "mock-key" => {
                format!("{key_var} is still the probe placeholder (mock-key): put a real key in the key field, or in the workspace .env")
            }
            Ok(_) => format!("the key is read from {key_var} — the workspace .env, or the shell that launched the app"),
            Err(_) => format!("{key_var} is not set: paste a key in the key field, or add one to the workspace .env, or every turn will fail"),
        };
        self.agent.conversation.notice(
            NoticeLevel::Info,
            format!("model route {route} · {chosen} is selected; {key_state}"),
        );
        self.restart_engine();
    }

    /// Deletes a route from the registry.
    ///
    /// @param route route key
    pub fn remove_route(&mut self, route: &str) {
        self.models.providers.remove(route);
        if self
            .models
            .active
            .as_ref()
            .is_some_and(|active| active.provider == route)
        {
            self.models.active = None;
            self.active_route = None;
        }
        let path = HarnessConfig::models_path_for(&self.root);
        match self.models.save(&path) {
            Ok(()) => {
                self.toast(format!("route {route} removed"));
                self.dialog = None;
            }
            Err(err) => self.toast(format!("could not write {}: {err}", path.display())),
        }
    }

    /// Drains everything the background subsystems produced.
    ///
    /// Called once per frame before drawing, so the interface is a function of
    /// state plus events rather than of timing.
    pub fn pump(&mut self) {
        self.pump_core();
        self.pump_agent();
        self.pump_search();
        self.watch_stuck_start();
        if self.task_dirty && self.task_saved_at.elapsed() >= TASK_SAVE_EVERY {
            self.save_task();
        }
    }

    /// Fails a turn that never got a session, instead of spinning forever.
    ///
    /// A first session commonly takes a few seconds; past this timeout, either
    /// the engine crashed without saying so, the route can never be reached, or
    /// the process is genuinely stuck — every one of those needs a visible
    /// failure and a retry, which is what every other failure in this app
    /// already offers. Without this, a turn that never gets a session spins
    /// its activity card ("starting the engine") with no way out but quitting
    /// the app.
    fn watch_stuck_start(&mut self) {
        if !self.agent.conversation.state.busy() || self.agent.conversation.session_id.is_some() {
            return;
        }
        let Some(elapsed) = self.agent.conversation.prompt_elapsed() else {
            return;
        };
        if elapsed < SESSION_START_TIMEOUT {
            return;
        }
        self.agent.pending_input = None;
        self.agent.conversation.fail(
            "the engine did not open a session in time. This usually means the route in ⌘, \
             cannot be reached, or the engine process is stuck. Try again, or restart the engine \
             (the title's ⋯ menu, or Restart the engine).",
        );
    }

    /// Adopts search results that belong to the newest request.
    fn pump_search(&mut self) {
        while let Ok((generation, query, outcome)) = self.search.results.try_recv() {
            if generation != self.search.generation {
                continue;
            }
            self.search.query_of_outcome = query;
            self.search.outcome = Some(outcome);
            self.search.pending = false;
            if let Some(palette) = self.palette.as_mut() {
                if palette.mode == PaletteMode::Contents {
                    palette.dirty = true;
                }
            }
        }
    }

    /// Drains terminal events.
    fn pump_core(&mut self) {
        let mut moved = false;
        while let Ok(event) = self.core.try_recv() {
            moved = true;
            match event {
                CoreEvent::BlockCompleted { id, .. } => {
                    if let Some(shell) = self.shells.iter_mut().find(|shell| shell.id == id) {
                        // The block list is read when the pane asks for it, so a
                        // finished command only marks it stale.
                        shell.blocks_dirty = true;
                    }
                }
                CoreEvent::TerminalExited { id, code } => {
                    log::info!("terminal {id:?} exited with {code:?}");
                }
                _ => {}
            }
        }
        if moved {
            self.last_activity = chrono::Local::now();
        }
    }

    /// Drains engine events into the conversation.
    fn pump_agent(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.agent.events.try_recv() {
            events.push(event);
        }
        if !events.is_empty() {
            self.last_activity = chrono::Local::now();
        }
        for event in events {
            // A session the window has left may still be finishing a cancelled
            // turn; its updates belong to its own saved task, not to this one.
            if let (Some(session), Some(current)) =
                (event.session(), self.agent.conversation.session_id.as_deref())
            {
                if session != current {
                    continue;
                }
            }
            match &event {
                AgentEvent::Initialized { name, version } => {
                    log::info!("engine: {name} {version}");
                    let workspace = self.root.to_string_lossy().into_owned();
                    if let Some(client) = self.agent.client.as_ref() {
                        let mcp = self.mcp_wire();
                        match self.agent.resume.take() {
                            Some(session) => {
                                client.resume_session(&session, &workspace, &mcp);
                            }
                            None => {
                                client.new_session_with(&workspace, &mcp);
                            }
                        }
                    }
                }
                AgentEvent::SessionReady { session_id, .. } => {
                    self.agent.live_sessions.insert(session_id.clone());
                    self.agent.conversation.apply(&event);
                    if self.apply_requested_model() {
                        self.send_pending_prompt();
                    }
                }
                AgentEvent::ConfigOptions { .. } => {
                    self.agent.conversation.apply(&event);
                    if self.apply_requested_model() {
                        self.send_pending_prompt();
                    }
                }
                AgentEvent::Request(AgentRequest::Permission {
                    rpc_id,
                    tool_call_id,
                    title,
                    reason,
                    options,
                    ..
                }) => {
                    let title = title
                        .clone()
                        .or_else(|| tool_call_id.clone())
                        .unwrap_or_else(|| "tool call".to_string());
                    // Auto approve answers with the narrowest allow option, so
                    // switching it on cannot silently widen permissions.
                    let auto = self
                        .agent
                        .auto_approve
                        .then(|| {
                            options
                                .iter()
                                .find(|option| option.kind == "allow_once")
                                .or_else(|| {
                                    options.iter().find(|option| option.kind == "allow_always")
                                })
                                .map(|option| (option.option_id.clone(), option.name.clone()))
                        })
                        .flatten();
                    match auto {
                        Some((option_id, name)) => {
                            if let Some(client) = self.agent.client.as_ref() {
                                client.respond_permission(rpc_id, Some(&option_id));
                            }
                            self.agent.conversation.notice(
                                NoticeLevel::Info,
                                format!("auto-approved {name}: {title}"),
                            );
                        }
                        None => self.agent.pending.push(PermissionAsk {
                            rpc_id: rpc_id.clone(),
                            title,
                            reason: reason.clone(),
                            options: options.clone(),
                        }),
                    }
                }
                AgentEvent::EngineExited { code } => {
                    self.agent
                        .conversation
                        .fail(format!("the engine exited ({code:?})"));
                    self.agent.client = None;
                }
                AgentEvent::RequestFailed { method, message }
                    if method.starts_with(harness_core::acp::RESUME_METHOD) =>
                {
                    // The engine could not reopen the saved session (its log
                    // was cleared, or it belongs to another checkout): the
                    // transcript is still the app's, and the next prompt runs
                    // in a fresh session.
                    log::warn!("engine: {method} failed: {message}");
                    self.agent.conversation.notice(
                        NoticeLevel::Warn,
                        "the engine could not reopen this task's session; the next prompt starts a fresh one",
                    );
                    self.agent.conversation.session_id = None;
                    if let Some(client) = self.agent.client.as_ref() {
                        client.new_session_with(&self.root.to_string_lossy(), &self.mcp_wire());
                    }
                    continue;
                }
                AgentEvent::RequestFailed { method, message } => {
                    log::warn!("engine: {method} failed: {message}");
                    if method == "session/prompt" || method == "session/new" || method == "session/set_config_option" {
                        // The engine's message is the provider's own error, JSON
                        // and all; the transcript gets the reason instead, and
                        // the run ends in one place rather than in two.
                        let text = describe_failure(message);
                        if let Some(queued) = self.agent.pending_input.take() {
                            self.agent.last_prompt = Some(queued);
                        }
                        self.agent.conversation.fail(text);
                        self.task_dirty = true;
                        self.save_task();
                        continue;
                    }
                    self.agent
                        .conversation
                        .notice(NoticeLevel::Warn, format!("{method}: {message}"));
                }
                _ => {}
            }
            let settled = matches!(event, AgentEvent::TurnEnded { .. } | AgentEvent::EngineExited { .. });
            if self.agent.conversation.apply(&event) {
                self.task_dirty = true;
            }
            if settled {
                // A finished turn is the natural checkpoint: the whole answer is in.
                self.save_task();
            }
        }
    }

    /// Tells the engine which model the workspace selected, when it can.
    ///
    /// The engine advertises its routes as opaque values; the app's registry
    /// knows which of them corresponds to the workspace's active route, so the
    /// selection is made by matching rather than by inventing a value.
    fn apply_requested_model(&mut self) -> bool {
        let Some(requested) = self.agent.requested_route.clone() else {
            return true;
        };
        let Some((route, model)) = requested.split_once('/') else {
            return false;
        };
        let Some(client) = self.agent.client.as_ref() else {
            return false;
        };
        let Some(session) = self.agent.conversation.session_id.clone() else {
            return false;
        };

        let advertised = self
            .agent
            .conversation
            .config_options
            .iter()
            .find(|option| option.id == "model");
        let Some(option) = advertised else { return false };
        let current = option.current_value.clone().unwrap_or_default();
        let matches = |choice: &ConfigChoice| choice_matches(&choice.value, route, model);
        let Some(choice) = option.choices.iter().find(|choice| matches(choice)) else {
            // The route is not mounted in this engine. A restart is the only
            // way to add one — the route registry is read at launch — so this
            // used to *say* "restarting it" without ever doing so, which left
            // the queued prompt stuck forever with no visible failure. Try the
            // restart exactly once per requested route, then report the
            // mismatch plainly so a route this build genuinely cannot serve
            // does not restart in a loop.
            if route_mismatch_action(self.agent.route_restart_attempted.as_deref(), &requested)
                == RouteMismatch::Restart
            {
                self.agent.route_restart_attempted = Some(requested.clone());
                // The stale queued text belongs to the engine being replaced;
                // restart_engine() fails the turn (if one is running), whose
                // "try again" resends the same text through send_prompt once
                // the new engine's session is ready.
                self.agent.pending_input = None;
                self.restart_engine();
                return false;
            }
            let message = format!(
                "the engine is not serving {requested} even after a restart — check the route in ⌘, (a model id the route's api does not know reads exactly like this)"
            );
            if self.agent.applied_model.as_deref() != Some(&message) {
                self.agent
                    .conversation
                    .notice(NoticeLevel::Warn, message.clone());
                self.agent.applied_model = Some(message);
            }
            return false;
        };
        if current == choice.value {
            self.agent.applied_model = Some(
                self.agent
                    .conversation
                    .active_model
                    .as_ref()
                    .map(|model| model.label.clone())
                    .unwrap_or_else(|| requested.clone()),
            );
            // A route that now resolves earns a fresh restart attempt if a
            // *later* mismatch turns up (e.g. the route document changes again).
            self.agent.route_restart_attempted = None;
            return true;
        }
        client.set_config_option(&session, "model", &choice.value);
        false
    }

    /// Dispatch a queued prompt only after the requested model is acknowledged.
    fn send_pending_prompt(&mut self) {
        if let (Some(client), Some(session)) = (
            self.agent.client.as_ref(), self.agent.conversation.session_id.as_deref(),
        ) {
            if let Some(text) = self.agent.pending_input.take() {
                client.prompt(session, &text);
                self.task_dirty = true;
            }
        }
    }
}

/// The parts of a `run_wave` call, as panel rows.
///
/// The call's input names every part; its result, once it arrives, reports one
/// line per part (`[wave N] id: done — summary`), which is where each row's own
/// outcome comes from. Any other call yields no rows.
///
/// @param card the delegation card
/// @returns one row per part, or nothing when the card is not a wave
fn wave_parts(card: &harness_core::agent::ToolCard) -> Vec<Delegation> {
    if !card.title.trim().eq_ignore_ascii_case("run_wave") {
        return Vec::new();
    }
    let Some(parts) = card
        .input
        .as_ref()
        .and_then(|input| input.get("parts"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| {
            let id = part.get("id")?.as_str()?.to_string();
            let task = part
                .get("task")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("a part of the job");
            let marker = format!("] {id}: ");
            let outcome = card
                .output
                .lines()
                .find_map(|line| line.split_once(marker.as_str()).map(|(_, rest)| rest.to_string()));
            let failed = outcome.as_deref().is_some_and(|rest| rest.starts_with("failed"))
                || (card.status == harness_core::agent::ToolStatus::Failed);
            Some(Delegation {
                agent: capitalize(&id),
                task: one_line(task, 96),
                call_id: card.id.clone(),
                done: outcome.is_some() || card.status.done(),
                failed,
            })
        })
        .collect()
}

/// The files one part of a `run_wave` call owned.
///
/// @param card the call
/// @param part the part's id, as its panel row names it (first letter raised)
/// @returns the part's declared files
fn wave_files(card: &harness_core::agent::ToolCard, part: &str) -> Vec<String> {
    card.input
        .as_ref()
        .and_then(|input| input.get("parts"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(part))
        })
        .flat_map(|entry| {
            entry
                .get("files")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|file| file.as_str().map(str::to_string))
        .collect()
}

/// What to do about a requested route the running engine does not advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteMismatch {
    /// Restart the engine: this route has not been tried against a fresh
    /// process yet, and the route registry is only read at launch.
    Restart,
    /// Report the mismatch plainly: a restart has already been tried for this
    /// exact route and it still does not appear, so trying again would only
    /// restart forever for a route this engine build genuinely cannot serve.
    Report,
}

/// Whether the active (or default) route can actually be asked anything, and
/// what to say when it cannot.
///
/// Checked before any engine or token is spent: a workspace nobody has
/// configured a real key for otherwise fails only after `ensure_engine`
/// starts a session, the model answers 401/429/whatever the free route's
/// shared pool says today, and — worse — if the prompt decomposes into
/// `run_wave` parts, several subagents each retry once against the same dead
/// route before the wave as a whole reports failed. All of that is avoidable:
/// the one fact that predicts every one of those failures, "is there a real
/// value behind the credential this route reads," is knowable for free before
/// any of it starts.
///
/// @param active the workspace's selected route, absent when none was ever chosen (the true first-run case)
/// @param providers the route profiles the selected route is looked up in
/// @param env reads one named variable's current value; a closure so tests need not touch real process env
/// @returns the message to fail the turn with, or nothing when the route has a usable key
fn missing_credential_message(
    active: Option<&harness_core::config::ActiveRoute>,
    providers: &std::collections::BTreeMap<String, harness_core::config::ProviderProfile>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let key_var = active
        .and_then(|active| providers.get(&active.provider))
        .and_then(|profile| profile.api_key_env.as_deref())
        .unwrap_or("AI_API_KEY");
    let usable = env(key_var).is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "mock-key"
    });
    if usable {
        return None;
    }
    let route_label = match active {
        Some(active) => format!("{}/{}", active.provider, active.model),
        None => "the free demo model".to_string(),
    };
    Some(format!(
        "{route_label} has no {key_var} set, so this can't actually run — nothing was spent trying. \
         Add a model and its key (\u{2318},), then press try again to resume this message."
    ))
}

/// Decides what a mismatched route earns: one restart, then a plain report.
///
/// Keyed on the route string itself (not a bare flag) so a *different* route
/// the user switches to afterward gets its own fresh attempt, and switching
/// back to a route that already resolved does too.
///
/// @param attempted the route a restart was already tried for, if any
/// @param requested the route this session wants right now
/// @returns what this mismatch earns
fn route_mismatch_action(attempted: Option<&str>, requested: &str) -> RouteMismatch {
    if attempted == Some(requested) {
        RouteMismatch::Report
    } else {
        RouteMismatch::Restart
    }
}

/// Whether an advertised model value is the route and model asked for.
///
/// The engine encodes a model choice as a JSON array of `[provider, model]`; the
/// app's registry keys routes by provider, so the comparison is structural and a
/// route the engine does not serve matches nothing.
///
/// @param value the advertised opaque value
/// @param route the provider key
/// @param model the model id
/// @returns true when they are the same route
fn choice_matches(value: &str, route: &str, model: &str) -> bool {
    match serde_json::from_str::<Vec<String>>(value) {
        Ok(parts) => parts.len() == 2 && parts[0] == route && parts[1] == model,
        Err(_) => false,
    }
}

/// Guesses a language from a path, for highlighting.
///
/// @param path the file
/// @returns a language id, `text` when nothing matches
pub fn language_of(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    match extension {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "go" => "go",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" => "markdown",
        "sh" | "bash" | "zsh" => "shell",
        "css" => "css",
        "html" => "html",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        _ if name.starts_with("Makefile") => "make",
        _ => "text",
    }
}

/// How many command hits a palette shows.
const COMMAND_HITS: usize = 40;

/// How many file hits a palette shows.
const FILE_HITS: usize = 80;

/// Orders command hits: fuzzy matches first, in score order, then the rest.
///
/// A palette that filters strictly is a palette that hides the command someone
/// opened it to find and does not remember the name of, so an unmatched command
/// stays in the list — below everything that matched.
///
/// @param hits the candidates
/// @param query what was typed
/// @param limit the most hits to keep
/// @returns the ordered hits
fn rank(hits: Vec<PaletteHit>, query: &str, limit: usize) -> Vec<PaletteHit> {
    if query.trim().is_empty() {
        let mut hits = hits;
        hits.truncate(limit);
        return hits;
    }
    let query = query.to_lowercase();
    let mut scored: Vec<(i32, usize, PaletteHit)> = hits
        .into_iter()
        .enumerate()
        .map(|(index, hit)| {
            let haystack = format!("{} {}", hit.label, hit.detail);
            let score = fsops::fuzzy_score(&query, &haystack).unwrap_or(i32::MIN);
            (score, index, hit)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored.into_iter().map(|(_, _, hit)| hit).collect()
}

/// Collapses a multi-line body into one line for a list.
///
/// @param text the text
/// @param limit the most characters to keep
/// @returns one line, elided when it was cut
fn one_line(text: &str, limit: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= limit {
        return joined;
    }
    let head: String = joined.chars().take(limit.saturating_sub(1)).collect();
    format!("{head}…")
}

/// A workflow draft, with every placeholder's default filled in.
///
/// @param workflow the workflow to edit
/// @param is_new whether it is a new one
/// @returns the draft
pub fn workflow_draft(workflow: Workflow, is_new: bool) -> WorkflowDraft {
    let mut values = BTreeMap::new();
    for variable in &workflow.variables {
        if let Some(default) = &variable.default {
            values.insert(variable.name.clone(), default.clone());
        }
    }
    let tags = workflow.tags.join(", ");
    WorkflowDraft {
        workflow,
        is_new,
        values,
        tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch workspace, empty at the start.
    ///
    /// @param name what the folder is for
    /// @returns the folder's path
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Writes a route document, the way the model dialog writes one.
    ///
    /// @param root the workspace
    fn write_route(root: &Path) {
        let harness = root.join(".harness");
        std::fs::create_dir_all(&harness).expect("harness dir");
        std::fs::write(
            harness.join("models.json"),
            r#"{"providers":{"deepseek":{"baseURL":"https://api.deepseek.com/v1","apiKeyEnv":"AI_API_KEY","models":[{"id":"deepseek-chat"}]}},"active":{"provider":"deepseek","model":"deepseek-chat"}}"#,
        )
        .expect("models document");
    }

    #[test]
    fn a_route_with_no_never_configured_active_route_reads_ai_api_key_and_names_itself_the_demo_model() {
        // The true first-run case: nobody has ever opened the model dialog, so
        // `active` is absent and the engine falls back to cordis.yml's own
        // default route, which reads AI_API_KEY.
        let providers = std::collections::BTreeMap::new();
        assert_eq!(missing_credential_message(None, &providers, |_| None), Some(
            "the free demo model has no AI_API_KEY set, so this can't actually run — nothing was spent trying. \
             Add a model and its key (\u{2318},), then press try again to resume this message.".to_string()
        ));
        assert_eq!(missing_credential_message(None, &providers, |name| (name == "AI_API_KEY").then(|| "sk-real".to_string())), None);
    }

    #[test]
    fn a_configured_route_is_checked_under_its_own_key_variable_not_ai_api_key() {
        let mut providers = std::collections::BTreeMap::new();
        providers.insert("Groq".to_string(), ProviderProfile {
            display_name: Some("Groq".to_string()),
            api: None,
            base_url: None,
            api_key_env: Some("GROQ_API_KEY".to_string()),
            models: None,
        });
        let active = harness_core::config::ActiveRoute { provider: "Groq".to_string(), model: "qwen/qwen3.8-27b".to_string() };
        // AI_API_KEY being set must not matter — this route reads its own variable.
        let env = |name: &str| (name == "AI_API_KEY").then(|| "unrelated".to_string());
        let message = missing_credential_message(Some(&active), &providers, env).expect("no GROQ_API_KEY is set");
        assert!(message.contains("Groq/qwen/qwen3.8-27b"));
        assert!(message.contains("GROQ_API_KEY"));
        let env = |name: &str| (name == "GROQ_API_KEY").then(|| "gsk_real".to_string());
        assert_eq!(missing_credential_message(Some(&active), &providers, env), None);
    }

    #[test]
    fn an_empty_or_placeholder_key_is_the_same_as_no_key_at_all() {
        let providers = std::collections::BTreeMap::new();
        assert!(missing_credential_message(None, &providers, |_| Some(String::new())).is_some());
        assert!(missing_credential_message(None, &providers, |_| Some("   ".to_string())).is_some());
        assert!(missing_credential_message(None, &providers, |_| Some("mock-key".to_string())).is_some());
    }

    #[test]
    fn opening_a_file_populates_the_relative_name_the_composer_chip_reads() {
        // The composer shows "editing: <name>" from `editor.buffer.relative`
        // whenever a file is open, so this is the one fact that chip depends
        // on: a real open must actually set it, to the workspace-relative
        // spelling (not the absolute path a long project root would make
        // unreadable in a narrow chip).
        let root = scratch("open-file-composer-chip");
        std::fs::write(root.join("todo.py"), "print('hi')\n").expect("file");
        let mut state = HarnessState::new(root.clone(), None);
        assert!(state.editor.buffer.is_none(), "nothing is open yet");
        state.open_file(&root.join("todo.py"));
        assert_eq!(state.editor.buffer.as_ref().map(|buffer| buffer.relative.as_str()), Some("todo.py"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opening_and_resaving_a_route_with_no_edits_keeps_its_context_window_and_max_tokens() {
        // Regression pin: a model saved with only `contextWindow` set (no
        // `name`) had its context window silently renamed into the `name`
        // field, and `maxTokens` was dropped outright, on a no-op
        // open-the-dialog-then-save — because the two fields were written to
        // the textarea positionally but only when present, so an absent
        // `name` shifted `contextWindow` one slot to the left. Two real
        // sessions hit exactly this: a rate-limit fix (contextWindow/maxTokens
        // set to fit a provider's per-minute cap) silently reverted the moment
        // the user reopened the model dialog to change something unrelated,
        // like the API key.
        let root = scratch("model-draft-roundtrip");
        let harness = root.join(".harness");
        std::fs::create_dir_all(&harness).expect("harness dir");
        std::fs::write(
            harness.join("models.json"),
            r#"{"providers":{"grok":{"baseURL":"https://api.groq.com/openai/v1","apiKeyEnv":"AI_API_KEY","models":[{"id":"qwen/qwen3.8-27b","contextWindow":5000,"maxTokens":1200}]}},"active":{"provider":"grok","model":"qwen/qwen3.8-27b"}}"#,
        )
        .expect("models document");
        let mut state = HarnessState::new(root.clone(), None);
        let draft = state.model_draft_for("grok");
        assert_eq!(
            draft.models, "qwen/qwen3.8-27b |  | 5000 | 1200",
            "the draft must carry both numbers forward, in their own fields, not shift them into name"
        );
        // Re-saving the unedited draft (what happens when a user opens the
        // dialog only to paste a new API key) must not lose either field.
        state.save_model_draft(&draft);
        let saved = state.models.providers.get("grok").expect("route still exists");
        let model = &saved.models.as_ref().expect("models kept")[0];
        assert_eq!(model.name, None, "no name was ever set; the round trip must not invent one");
        assert_eq!(model.context_window, Some(5000));
        assert_eq!(model.max_tokens, Some(1200));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn daily_quota_invalid_endpoint_and_stream_failures_have_distinct_repairs() {
        let daily = describe_failure(r#"429: {"message":"Rate limit exceeded: free-models-per-day","metadata":{"headers":{"X-RateLimit-Reset":"1790467200000"}}}"#);
        assert!(daily.contains("daily free-model quota"));
        assert!(daily.contains("2026-09-27 00:00 UTC"));
        assert!(!daily.contains("Try again in a moment"));
        assert!(describe_failure("Invalid URL").contains("complete API base URL"));
        assert!(describe_failure("Provider finish_reason: error").contains("supplied no further detail"));
    }

    #[test]
    fn route_restart_keeps_context_and_discards_retired_engine_events() {
        let root = scratch("route-restart");
        let mut state = HarnessState::new(root.clone(), None);
        // Prevent launching an actual engine in this state regression.
        state.agent.starting = true;
        state.agent.conversation.begin_prompt("Remember the original task");
        state.agent.conversation.session_id = Some("durable-session".into());
        state.agent.live_sessions.insert("durable-session".into());
        let retired = state.agent.sender.clone();
        state.restart_engine();
        assert_eq!(state.agent.resume.as_deref(), Some("durable-session"));
        assert!(state.agent.conversation.session_id.is_none());
        assert!(!state.agent.conversation.state.busy());
        assert!(state.agent.live_sessions.is_empty());
        assert!(retired.send(AgentEvent::EngineExited { code: Some(0) }).is_err());
        assert_eq!(state.agent.conversation.first_prompt(), Some("Remember the original task"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_mismatched_route_earns_exactly_one_restart_then_a_plain_report() {
        // This is the bug behind a turn that spins on "starting the engine"
        // forever: the route-mismatch branch used to *say* "restarting it"
        // without calling anything, so a route the freshly-launched engine
        // never advertised (a typo'd model id, a route document written after
        // the engine already started) queued a prompt that was never sent and
        // never failed either — nothing told the user it was stuck.
        assert_eq!(
            route_mismatch_action(None, "deepseek/deepseek-chat"),
            RouteMismatch::Restart,
            "the first mismatch for a route always restarts"
        );
        assert_eq!(
            route_mismatch_action(Some("deepseek/deepseek-chat"), "deepseek/deepseek-chat"),
            RouteMismatch::Report,
            "a second mismatch for the SAME route must not restart again — a route this engine \
             build genuinely cannot serve would otherwise restart forever with no visible failure"
        );
        assert_eq!(
            route_mismatch_action(Some("deepseek/deepseek-chat"), "qwen/qwen3.7-plus"),
            RouteMismatch::Restart,
            "switching to a DIFFERENT route earns its own fresh attempt"
        );
    }

    #[test]
    fn a_provider_error_becomes_a_sentence_with_the_way_out() {
        // Verbatim from the run that reported it: a free OpenRouter route whose
        // shared pool was exhausted. The transcript must not show this JSON.
        let message = r#"Internal error: turn failed: 429: {"message":"Provider returned error","code":429,"metadata":{"raw":"poolside/laguna-xs-2.1:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits.","provider_name":"Poolside","is_byok":false,"limit_source":"upstream_provider_shared_pool"}}"#;
        let described = describe_failure(message);
        assert!(described.contains("rate-limited"), "{described}");
        assert!(described.contains("⌘,"), "it says where to change route: {described}");
        assert!(
            described.contains("temporarily rate-limited upstream"),
            "the provider's own hint survives: {described}"
        );
        assert!(
            !described.contains("\"metadata\""),
            "no JSON reaches the transcript: {described}"
        );
    }

    #[test]
    fn the_other_provider_errors_name_their_own_repair() {
        let unauthorized = describe_failure("401: {\"message\":\"invalid api key\"}");
        assert!(unauthorized.contains("rejected the key"), "{unauthorized}");
        assert!(unauthorized.contains("⌘,"), "{unauthorized}");

        let missing = describe_failure("404: page not found");
        assert!(missing.contains("base URL or the model id"), "{missing}");

        let no_adapter = describe_failure("provider \"mock\" needs an api");
        assert!(no_adapter.contains("openai-completions"), "{no_adapter}");

        let offline = describe_failure("fetch failed");
        assert!(offline.contains("could not be reached"), "{offline}");

        // Anything unrecognised still says what failed, with the body shortened.
        let unknown = describe_failure(&format!("boom {}", "x".repeat(500)));
        assert!(unknown.starts_with("the turn failed"), "{unknown}");
        assert!(unknown.chars().count() < 260, "kept short: {}", unknown.len());
    }

    #[test]
    fn a_task_is_kept_reopened_whole_and_reachable_by_back_and_forward() {
        let root = scratch("history");
        let mut state = HarnessState::new(root.clone(), None);
        // No engine process in a unit test: the resume is recorded, not sent.
        state.paths = Err("no engine in unit tests".to_string());
        assert!(state.tasks.is_empty(), "a new workspace has no history");

        // Nothing asked is not history.
        state.save_task();
        assert!(state.tasks.is_empty());

        state.agent.conversation.begin_prompt("Build a todo CLI with tests");
        state.agent.conversation.session_id = Some("session-one".to_string());
        state.agent.conversation.notice(NoticeLevel::Info, "answered");
        // The turn finished; a running one would be stopped (and say so) on leaving.
        state.agent.conversation.state = RunState::Idle;
        state.current_task = Some(TaskStore::new_id());
        state.save_task();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].title, "Build a todo CLI with tests");
        let first = state.current_task.clone().expect("the task has an id");

        // A new task keeps the old one in the history, and starts empty.
        state.new_task();
        assert!(state.current_task.is_none());
        assert!(state.agent.conversation.items.is_empty());
        assert_eq!(state.tasks.len(), 1, "the first task survived the new one");

        // Reopening restores the transcript and remembers the engine session,
        // which the engine is asked to resume once it is up.
        state.open_task(&first);
        assert_eq!(state.current_task.as_deref(), Some(first.as_str()));
        assert_eq!(state.agent.conversation.items.len(), 2);
        assert_eq!(state.agent.resume.as_deref(), Some("session-one"));
        assert_eq!(
            state.agent.last_prompt.as_deref(),
            Some("Build a todo CLI with tests"),
            "try again asks the reopened task's own last prompt"
        );

        // Back returns to the new task; forward returns to the reopened one.
        assert!(state.can_go_back());
        state.navigate(false);
        assert!(state.current_task.is_none() || state.current_task.as_deref() != Some(first.as_str()));
        state.navigate(true);
        assert_eq!(state.current_task.as_deref(), Some(first.as_str()));

        // Rename sticks through later saves; delete removes it from the history.
        state.rename_task(&first, "Todo CLI");
        state.save_task();
        assert_eq!(state.tasks[0].title, "Todo CLI");
        assert_eq!(state.session_title(), "Todo CLI");
        state.delete_task(&first);
        assert!(state.tasks.is_empty());
        assert!(state.current_task.is_none());
    }

    #[test]
    fn switching_workspace_moves_the_session_and_carries_the_route() {
        let first = scratch("first");
        let second = scratch("second");
        write_route(&first);

        let mut state = HarnessState::new(first.clone(), None);
        assert_eq!(state.root, first);
        assert_eq!(
            state.active_route.as_deref(),
            Some("deepseek/deepseek-chat")
        );

        state.switch_workspace(second.clone());
        assert_eq!(state.root, second, "the folder is the new one");
        assert_eq!(
            state.active_route.as_deref(),
            Some("deepseek/deepseek-chat"),
            "a folder with no route document keeps the route that was working"
        );
        let carried = ModelsDocument::load(&HarnessConfig::models_path_for(&second))
            .expect("carried document");
        assert_eq!(
            carried.active, state.models.active,
            "and the carried document is on disk"
        );
        assert!(
            state
                .agent
                .conversation
                .items
                .iter()
                .any(|item| matches!(item, harness_core::agent::Item::Notice { .. })),
            "the fresh session says where it is working"
        );
        assert!(
            state
                .shells
                .iter()
                .all(|shell| shell.terminal.is_some() || shell.error.is_some()),
            "a shell was asked for in the new folder"
        );
        state.agent.client = None; // dropping the client ends the engine process

        // The same folder, and a file, are both refused rather than opened.
        state.switch_workspace(second.clone());
        assert_eq!(state.root, second);
        let file = second.join("a-file");
        std::fs::write(&file, "x").expect("file");
        state.switch_workspace(file);
        assert_eq!(state.root, second);

        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn the_folder_picker_walks_directories_and_offers_shortcuts() {
        let root = scratch("picker");
        std::fs::create_dir_all(root.join("inner")).expect("inner");
        std::fs::write(root.join("readme.md"), "x").expect("file");

        let mut draft = FolderDraft::new(root.clone());
        let dirs = draft.dirs().expect("listing");
        assert!(dirs.iter().any(|(name, _, _)| name == "inner"));
        assert!(
            !dirs.iter().any(|(name, _, _)| name == "readme.md"),
            "a workspace is a folder, so files are not rows"
        );

        draft.enter(root.join("inner"));
        assert_eq!(draft.dir, root.join("inner"));
        draft.up();
        assert_eq!(draft.dir, root);
        draft.enter(root.join("readme.md"));
        assert!(draft.error.is_some(), "a file is refused, with a reason");

        let shortcuts = folder_shortcuts(&root, None);
        assert!(
            shortcuts
                .iter()
                .any(|(label, path)| label == "current workspace" && path == &root),
            "the folder already open is always one of the shortcuts"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
