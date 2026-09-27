//! The agent session model: one conversation, as the UI sees it.
//!
//! The ACP client ([`crate::acp`]) delivers a stream of [`AgentEvent`]s. This
//! module is the reducer that turns them into a transcript a pane can draw:
//! user and assistant bubbles, thinking, tool cards with their output, the
//! model's plan, run state, and token accounting. It is deliberately pure — no
//! I/O, no threads — so the whole session model is testable from fixtures and
//! the UI stays a function of state.
//!
//! Two properties matter for the product's token discipline:
//!
//! - **Context is measured, not guessed.** The engine reports the context
//!   window's occupancy, and that is what the context bar shows.
//! - **Session totals are labelled estimates.** Over ACP the engine reports
//!   occupancy, not a per-turn token bill, so the transcript's own token counts
//!   are heuristics and the UI says so.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::acp::{AgentEvent, ConfigChoice, ConfigOption, PlanEntry, ToolCall};
use crate::config::Price;
use crate::event::ConversationId;

/// Characters per estimated token for the transcript heuristic.
const CHARS_PER_TOKEN: u64 = 4;

/// How many engine log lines to keep for the diagnostics pane.
const LOG_LINES: usize = 500;

/// What the conversation is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// No prompt is in flight.
    Idle,
    /// A prompt is running; the engine is working.
    Running,
    /// The engine is blocked on an approval the app must answer.
    AwaitingApproval {
        /// Title of the call being approved, for the prompt.
        title: String,
    },
    /// The engine exited or a request failed; nothing more will run.
    Failed {
        /// What went wrong.
        message: String,
    },
}

impl RunState {
    /// Whether a prompt is in flight (used to enable Stop).
    ///
    /// @returns true while the engine is working or waiting on the app
    pub fn busy(&self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval { .. })
    }

    /// Short label for the status pill.
    ///
    /// @returns label text
    pub fn label(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Running => "working",
            Self::AwaitingApproval { .. } => "approval",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Lifecycle of one tool call card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    /// Announced, not started.
    Pending,
    /// Running.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
}

impl ToolStatus {
    /// Reads the protocol's status string.
    ///
    /// @param status `pending`, `in_progress`, `completed`, `failed`
    /// @returns the matching status, `Running` for anything unrecognized
    pub fn parse(status: &str) -> Self {
        match status {
            "pending" => Self::Pending,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            // `in_progress` and anything a newer engine adds: a call that is
            // not finished is running, which is the safe way to draw it.
            _ => Self::Running,
        }
    }

    /// Whether the call is finished.
    ///
    /// @returns true for completed and failed
    pub fn done(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Short label for the card header.
    ///
    /// @returns label text
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "queued",
            Self::Running => "running",
            Self::Completed => "done",
            Self::Failed => "failed",
        }
    }
}

/// Tool names that mean "work handed to another agent".
const DELEGATION_TOOLS: [&str; 6] = [
    "subagent",
    "subagent_fork",
    "task",
    "agent",
    "committee",
    "run_wave",
];

/// Tool names that mean "a command ran".
const COMMAND_TOOLS: [&str; 6] = [
    "bash",
    "pwsh",
    "bash_persistent",
    "pwsh_persistent",
    "shell",
    "execute",
];

/// What a file-changing call did, as the two lists the card draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The file the call named.
    pub path: String,
    /// Lines the call removed.
    pub removed: Vec<String>,
    /// Lines the call added.
    pub added: Vec<String>,
}

/// One tool call, as the card draws it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCard {
    /// Protocol call id, the key updates arrive under.
    pub id: String,
    /// Title shown on the card.
    pub title: String,
    /// Category: `read`, `edit`, `execute`, `search`, `fetch`, `other`.
    pub kind: String,
    /// Lifecycle.
    pub status: ToolStatus,
    /// Raw input, pretty-printed on demand.
    pub input: Option<Value>,
    /// Output text, appended as updates arrive.
    pub output: String,
    /// Files the call touched, for follow-along.
    pub locations: Vec<String>,
    /// Milliseconds from announcement to completion, once known.
    pub elapsed_ms: Option<u64>,
}

impl ToolCard {
    /// Whether the card is a harness escalation (a budget ask).
    ///
    /// The preset brands its escalation call so the app can present it as a
    /// budget decision rather than a generic tool approval.
    ///
    /// @returns true when this card is the harness's own budget escalation
    pub fn is_budget_escalation(&self) -> bool {
        self.title.contains("harness_budget") || self.id.contains("harness_budget")
    }

    /// Whether the call is work handed to another agent.
    ///
    /// dsh names its delegation tools per deployment — `subagent`, its fork
    /// variant, the `task` and `agent` aliases — and the preset adds
    /// `committee`; to the interface they all mean one thing: a second agent is
    /// doing part of the job, and what comes back is its answer.
    ///
    /// @returns true when the call is a delegation
    pub fn is_delegation(&self) -> bool {
        let name = self.title.trim().to_ascii_lowercase();
        DELEGATION_TOOLS.contains(&name.as_str()) || name.starts_with("subagent")
    }

    /// Whether the call ran a command.
    ///
    /// @returns true when the call is a command
    pub fn is_command(&self) -> bool {
        COMMAND_TOOLS.contains(&self.title.trim().to_ascii_lowercase().as_str())
    }

    /// Whether the call is the engine writing down its task list.
    ///
    /// dsh's `todo_write` replaces the whole list on every call, so the last
    /// one in a session is the current list — which is why the transcript
    /// draws one live card for it instead of a row per call.
    ///
    /// @returns true when the call is a `todo_write`
    pub fn is_todo_write(&self) -> bool {
        self.title.trim().eq_ignore_ascii_case("todo_write")
    }

    /// The task list this call wrote.
    ///
    /// @returns the (content, status) pairs; empty when this is not a todo call
    pub fn todos(&self) -> Vec<(String, String)> {
        let Some(Value::Object(map)) = self.input.as_ref() else {
            return Vec::new();
        };
        let Some(Value::Array(items)) = map.get("todos") else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| {
                let content = item
                    .get("content")
                    .and_then(Value::as_str)?
                    .trim()
                    .to_string();
                let status = item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .to_string();
                (!content.is_empty()).then_some((content, status))
            })
            .collect()
    }

    /// The change this call made, when it changed a file.
    ///
    /// dsh offers three file-writing tools and each spells its arguments
    /// differently: `edit` replaces one literal, `write` states the whole new
    /// content, and `str_replace_editor` dispatches on its `command`. This is
    /// the one place that difference is known.
    ///
    /// @returns the change, or None when the call changed nothing
    pub fn change(&self) -> Option<FileChange> {
        let Some(Value::Object(map)) = self.input.as_ref() else {
            return None;
        };
        let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_string);
        let lines = |text: Option<String>| {
            text.map(|text| text.lines().map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        match self.title.trim().to_ascii_lowercase().as_str() {
            "edit" => Some(FileChange {
                path: text("file_path").unwrap_or_default(),
                removed: lines(text("old_string")),
                added: lines(text("new_string")),
            }),
            "write" => Some(FileChange {
                path: text("file_path").unwrap_or_default(),
                removed: Vec::new(),
                added: lines(text("content")),
            }),
            "str_replace_editor" => {
                let path = text("path").unwrap_or_default();
                match text("command").as_deref() {
                    Some("create") => Some(FileChange {
                        path,
                        removed: Vec::new(),
                        added: lines(text("file_text")),
                    }),
                    Some("insert") => Some(FileChange {
                        path,
                        removed: Vec::new(),
                        added: lines(text("new_str")),
                    }),
                    Some("str_replace") => Some(FileChange {
                        path,
                        removed: lines(text("old_str")),
                        added: lines(text("new_str")),
                    }),
                    // `view` reads; it changes nothing.
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// One named field of the call's input, as trimmed text.
    ///
    /// @param keys the field names to try, in order
    /// @returns the first field that holds a non-empty string
    pub fn input_text(&self, keys: &[&str]) -> Option<String> {
        let Some(Value::Object(map)) = self.input.as_ref() else {
            return None;
        };
        for key in keys {
            if let Some(text) = map.get(*key).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        None
    }

    /// One-line summary for a collapsed card.
    ///
    /// A call that has answered says so in its own words. One that has not is
    /// described by the argument that says what it is about — the command, the
    /// pattern, the file — rather than by how many arguments it was passed.
    ///
    /// @returns the summary line
    pub fn summary(&self) -> String {
        if let Some(line) = self.output.lines().find(|line| !line.trim().is_empty()) {
            return line.trim().to_string();
        }
        if let Some(text) = self.subject() {
            return text;
        }
        match &self.input {
            Some(Value::Object(map)) => format!("{} field(s)", map.len()),
            Some(input) => input.to_string(),
            None => String::new(),
        }
    }

    /// The string argument that names what this call acts on.
    ///
    /// @returns the first argument of interest that holds text
    fn subject(&self) -> Option<String> {
        let Some(Value::Object(map)) = self.input.as_ref() else {
            return None;
        };
        for key in [
            "command",
            "pattern",
            "path",
            "file",
            "query",
            "url",
            "description",
            "prompt",
            "task",
            "name",
            "text",
        ] {
            let Some(text) = map.get(key).and_then(Value::as_str) else {
                continue;
            };
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.lines().next().unwrap_or(text).to_string());
            }
        }
        None
    }
}

/// Severity of a transcript notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeLevel {
    /// Neutral information.
    Info,
    /// Something the user should see but that does not stop work.
    Warn,
    /// A failure.
    Error,
}

/// One transcript entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Item {
    /// The user's own message.
    User {
        /// Bubble id.
        id: String,
        /// Text.
        text: String,
    },
    /// An assistant message, with its thinking kept alongside.
    Assistant {
        /// Bubble id.
        id: String,
        /// Answer text.
        text: String,
        /// Reasoning text, when the route emits a thought channel.
        thinking: String,
        /// Whether the thought is still streaming (drawn open).
        thinking_open: bool,
        /// What the user thought of it: `1` useful, `-1` not, `None` unrated.
        rating: Option<i8>,
    },
    /// A tool call card.
    Tool(ToolCard),
    /// A notice: an engine problem, an approval decision, a lifecycle remark.
    Notice {
        /// Severity.
        level: NoticeLevel,
        /// Text.
        text: String,
    },
}

impl Item {
    /// Whether the item is a transcript bubble the user wrote or read.
    ///
    /// @returns true for user and assistant items
    pub fn is_bubble(&self) -> bool {
        matches!(self, Self::User { .. } | Self::Assistant { .. })
    }
}

/// Which stream an incoming chunk belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Chunk {
    /// Assistant answer text.
    Assistant,
    /// Assistant reasoning.
    Thought,
    /// The user's message, when the engine echoes it.
    User,
}

/// The context window and session accounting.
#[derive(Debug, Clone, Default)]
pub struct TokenMeter {
    /// Tokens currently occupying the context window, as the engine reports.
    pub used: u64,
    /// The window's size.
    pub size: u64,
    /// Highest occupancy seen this session (the number that decides compaction).
    pub peak_used: u64,
    /// How many usage reports arrived.
    pub updates: u64,
    /// Estimated prompt-side tokens from the transcript itself.
    pub prompt_tokens_estimate: u64,
    /// Estimated completion-side tokens from the transcript itself.
    pub completion_tokens_estimate: u64,
    /// Prices for the active model, when the user supplied them.
    pub price: Option<Price>,
}

impl TokenMeter {
    /// Records one usage report.
    ///
    /// @param used tokens in the context window
    /// @param size the window's size
    pub fn observe(&mut self, used: u64, size: u64) {
        self.used = used;
        if size > 0 {
            self.size = size;
        }
        self.peak_used = self.peak_used.max(used);
        self.updates += 1;
    }

    /// Adds estimated prompt-side tokens (the user's own text and tool output).
    ///
    /// @param text text that will travel to the model
    pub fn count_prompt(&mut self, text: &str) {
        self.prompt_tokens_estimate += estimate_tokens(text);
    }

    /// Adds estimated completion-side tokens (assistant text and thinking).
    ///
    /// @param text text the model produced
    pub fn count_completion(&mut self, text: &str) {
        self.completion_tokens_estimate += estimate_tokens(text);
    }

    /// Fraction of the context window in use.
    ///
    /// @returns 0.0–1.0, or 0.0 before the first report
    pub fn context_fraction(&self) -> f32 {
        if self.size == 0 {
            return 0.0;
        }
        (self.used as f32 / self.size as f32).clamp(0.0, 1.0)
    }

    /// Estimated total tokens the session has moved.
    ///
    /// @returns prompt plus completion estimates
    pub fn estimated_total(&self) -> u64 {
        self.prompt_tokens_estimate + self.completion_tokens_estimate
    }

    /// Estimated session cost, when a price table is known.
    ///
    /// @returns USD, or `None` without prices
    pub fn estimated_cost_usd(&self) -> Option<f64> {
        let price = self.price?;
        let input = self.prompt_tokens_estimate as f64 / 1_000_000.0 * price.input;
        let output = self.completion_tokens_estimate as f64 / 1_000_000.0 * price.output;
        Some(input + output)
    }
}

/// Estimates tokens in a string.
///
/// Four characters per token is the usual English approximation and is only
/// ever shown as an estimate — the engine's own context figure is the measured
/// number.
///
/// @param text text to estimate
/// @returns token estimate, rounded up
pub fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(CHARS_PER_TOKEN)
}

/// One conversation: transcript, run state, plan, and accounting.
#[derive(Debug)]
pub struct Conversation {
    /// Identity of this conversation within the app.
    pub id: ConversationId,
    /// The engine's session id, once a session exists.
    pub session_id: Option<String>,
    /// Engine name and version, as reported by `initialize`.
    pub engine_name: String,
    /// Engine version.
    pub engine_version: String,
    /// Current run state.
    pub state: RunState,
    /// The transcript, in order.
    pub items: Vec<Item>,
    /// The model's own plan, replaced whole on every update.
    pub plan: Vec<PlanEntry>,
    /// Context window and token accounting.
    pub meter: TokenMeter,
    /// Session configuration options, as advertised.
    pub config_options: Vec<ConfigOption>,
    /// Currently selected model choice, when one is identifiable.
    pub active_model: Option<ConfigChoice>,
    /// Recent engine log lines, oldest first.
    pub log: VecDeque<String>,
    /// Count of protocol notifications the app does not model yet.
    pub unhandled: BTreeMap<String, u32>,
    /// Index of the assistant bubble currently streaming, if any.
    open_assistant: Option<usize>,
    /// Answer message id streaming into that bubble, when one is.
    open_answer_id: Option<String>,
    /// Index of the bubble open for each user stream.
    user_streams: HashMap<String, usize>,
    /// Index of every tool card by call id.
    cards: HashMap<String, usize>,
    /// When the current prompt was sent.
    prompt_started: Option<std::time::Instant>,
}

impl Conversation {
    /// Creates an empty conversation.
    ///
    /// @param id identity within the app
    /// @returns a conversation in [`RunState::Idle`]
    pub fn new(id: ConversationId) -> Self {
        Self {
            id,
            session_id: None,
            engine_name: String::new(),
            engine_version: String::new(),
            state: RunState::Idle,
            items: Vec::new(),
            plan: Vec::new(),
            meter: TokenMeter::default(),
            config_options: Vec::new(),
            active_model: None,
            log: VecDeque::new(),
            unhandled: BTreeMap::new(),
            open_assistant: None,
            open_answer_id: None,
            user_streams: HashMap::new(),
            cards: HashMap::new(),
            prompt_started: None,
        }
    }

    /// Rebuilds a conversation from a saved transcript, so a task opened from
    /// history reads exactly as it did — and its tool cards keep taking updates
    /// if the engine resumes the session they belong to.
    ///
    /// Streams are closed and the run is idle: whatever was in flight when the
    /// task was saved is over, and a restored card still marked running is
    /// shown as failed rather than spinning forever.
    ///
    /// @param id identity within the app
    /// @param session_id the engine session the transcript belongs to, when known
    /// @param items the saved transcript
    /// @returns the restored conversation
    pub fn restore(id: ConversationId, session_id: Option<String>, mut items: Vec<Item>) -> Self {
        let mut conversation = Self::new(id);
        for item in &mut items {
            if let Item::Tool(card) = item {
                if !card.status.done() {
                    card.status = ToolStatus::Failed;
                }
            }
            if let Item::Assistant { thinking_open, .. } = item {
                *thinking_open = false;
            }
        }
        for (index, item) in items.iter().enumerate() {
            if let Item::Tool(card) = item {
                conversation.cards.insert(card.id.clone(), index);
            }
        }
        conversation.session_id = session_id;
        conversation.items = items;
        conversation
    }

    /// The first thing the user asked, which is what a task is called.
    ///
    /// @returns the first user message, or `None` before one was sent
    pub fn first_prompt(&self) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }

    /// Records the user's prompt locally, so the transcript shows it before the
    /// engine answers (the engine does not echo prompts back).
    ///
    /// @param text the message sent
    pub fn begin_prompt(&mut self, text: &str) {
        self.items.push(Item::User {
            id: format!("local-{}", self.items.len()),
            text: text.to_string(),
        });
        self.meter.count_prompt(text);
        self.state = RunState::Running;
        self.prompt_started = Some(std::time::Instant::now());
        // Each turn streams into its own bubble, so nothing carries over.
        self.close_bubbles();
    }

    /// How long the current prompt has been running.
    ///
    /// @returns elapsed time, or `None` when idle
    pub fn prompt_elapsed(&self) -> Option<std::time::Duration> {
        self.prompt_started.map(|started| started.elapsed())
    }

    /// Records what the user thought of one answer.
    ///
    /// The rating is the app's own annotation: nothing is sent to the engine,
    /// and the export carries it so a transcript read later says which answers
    /// were worth keeping.
    ///
    /// @param id the bubble's id, as the transcript draws it
    /// @param rating `1` useful, `-1` not useful, `None` to clear
    /// @returns whether a bubble with that id was found
    pub fn rate(&mut self, id: &str, rating: Option<i8>) -> bool {
        for item in &mut self.items {
            if let Item::Assistant {
                id: bubble,
                rating: stored,
                ..
            } = item
            {
                if bubble == id {
                    *stored = rating;
                    return true;
                }
            }
        }
        false
    }

    /// Records an approval decision the user made, in the transcript.
    ///
    /// @param title what was approved or refused
    /// @param decision the option name chosen, or `None` for a cancel
    pub fn record_decision(&mut self, title: &str, decision: Option<&str>) {
        let text = match decision {
            Some(name) => format!("{title} — {name}"),
            None => format!("{title} — dismissed"),
        };
        self.items.push(Item::Notice {
            level: if decision.is_some() {
                NoticeLevel::Info
            } else {
                NoticeLevel::Warn
            },
            text,
        });
        if self.state.busy() {
            self.state = RunState::Running;
        }
    }

    /// Adds a notice of the app's own making (a launcher failure, a refusal).
    ///
    /// @param level severity
    /// @param text message
    pub fn notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.items.push(Item::Notice {
            level,
            text: text.into(),
        });
    }

    /// Fails the conversation and says why.
    ///
    /// @param message what went wrong
    pub fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.items.push(Item::Notice {
            level: NoticeLevel::Error,
            text: message.clone(),
        });
        self.state = RunState::Failed { message };
    }

    /// Reduces one engine event into the transcript.
    ///
    /// @param event the event, as delivered by the ACP client
    /// @returns whether the transcript, plan, or state changed visibly
    pub fn apply(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::Initialized { name: _, version: _ } => {
                // The wire's own `agentInfo` names the vendored engine directly
                // (its ACP handshake hardcodes its own package name and an
                // internal version that is never bumped); every place that
                // reads `engine_name`/`engine_version` is a label the user
                // reads (the footer, the turn-failed card, the observability
                // panel, the exported transcript), so it carries this
                // application's own identity instead of the dependency's.
                self.engine_name = "AI Harness engine".to_string();
                self.engine_version = env!("CARGO_PKG_VERSION").to_string();
                true
            }
            AgentEvent::SessionReady {
                session_id,
                config_options,
            } => {
                self.session_id = Some(session_id.clone());
                self.set_config_options(config_options);
                true
            }
            AgentEvent::ConfigOptions { config_options, .. } => {
                self.set_config_options(config_options);
                true
            }
            AgentEvent::AgentMessage {
                message_id, text, ..
            } => {
                self.append_chunk(Chunk::Assistant, message_id, text);
                self.meter.count_completion(text);
                true
            }
            AgentEvent::AgentThought {
                message_id, text, ..
            } => {
                self.append_chunk(Chunk::Thought, message_id, text);
                self.meter.count_completion(text);
                true
            }
            AgentEvent::UserMessage {
                message_id, text, ..
            } => {
                self.append_chunk(Chunk::User, message_id, text);
                true
            }
            AgentEvent::ToolCall { call, .. } => {
                self.push_card(call);
                if !matches!(self.state, RunState::AwaitingApproval { .. }) {
                    self.state = RunState::Running;
                }
                true
            }
            AgentEvent::ToolCallUpdate {
                tool_call_id,
                title,
                status,
                output,
                input,
                locations,
                ..
            } => {
                self.update_card(
                    tool_call_id,
                    title.as_deref(),
                    status.as_deref(),
                    output.as_deref(),
                    input.clone(),
                    locations,
                );
                true
            }
            AgentEvent::Plan { entries, .. } => {
                self.plan = entries.clone();
                true
            }
            AgentEvent::Usage { used, size, .. } => {
                self.meter.observe(*used, *size);
                true
            }
            AgentEvent::TurnEnded { stop_reason, .. } => {
                self.close_bubbles();
                if stop_reason == "end_turn" || stop_reason == "max_tokens" {
                    self.state = RunState::Idle;
                } else if stop_reason == "refusal" {
                    self.notice(NoticeLevel::Warn, "the model refused this turn");
                    self.state = RunState::Idle;
                } else {
                    // `cancelled` and anything new: the turn stopped because the
                    // app or the engine said so, not because the model finished.
                    self.notice(NoticeLevel::Info, format!("turn {stop_reason}"));
                    self.state = RunState::Idle;
                }
                self.prompt_started = None;
                true
            }
            AgentEvent::Request(crate::acp::AgentRequest::Permission {
                title,
                tool_call_id,
                ..
            }) => {
                let label = title
                    .clone()
                    .or_else(|| tool_call_id.clone())
                    .unwrap_or_else(|| "a tool call".to_string());
                self.state = RunState::AwaitingApproval { title: label };
                true
            }
            AgentEvent::RequestFailed { method, message } => {
                // A failed prompt or session is the end of the run, not a remark
                // about it: the turn will never finish, so leaving the state at
                // Running would keep the spinner turning and the app repainting
                // for work that is already over.
                if method == "session/prompt" || method == "session/new" {
                    self.fail(message.clone());
                } else {
                    self.notice(NoticeLevel::Error, format!("{method} failed: {message}"));
                }
                true
            }
            AgentEvent::Unhandled { method } => {
                *self.unhandled.entry(method.clone()).or_insert(0) += 1;
                self.push_log(format!("unhandled notification: {method}"));
                true
            }
            AgentEvent::EngineLog { line } => {
                self.push_log(line.clone());
                // Log lines stream constantly; they redraw the diagnostics pane
                // but are not transcript changes.
                false
            }
            AgentEvent::EngineExited { code } => {
                let message = match code {
                    Some(code) => format!("the agent engine exited with status {code}"),
                    None => "the agent engine exited".to_string(),
                };
                self.close_bubbles();
                self.fail(message);
                true
            }
        }
    }

    /// Keeps one log line, bounded.
    fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// Applies a full configuration list, keeping the selected model.
    fn set_config_options(&mut self, options: &[ConfigOption]) {
        self.config_options = options.to_vec();
        self.active_model = options
            .iter()
            .find(|option| option.id == "model")
            .and_then(|option| {
                option
                    .choices
                    .iter()
                    .find(|choice| Some(&choice.value) == option.current_value.as_ref())
                    .cloned()
            });
    }

    /// Routes one streaming chunk into the transcript.
    ///
    /// An answer and the reasoning that produced it share one bubble: the
    /// engine streams them as two message ids, but a person reads them as one
    /// turn, and a second answer message inside the same turn starts a new
    /// bubble only once the first has text.
    fn append_chunk(&mut self, stream: Chunk, message_id: &str, text: &str) {
        match stream {
            Chunk::Thought => self.append_thought(text),
            Chunk::Assistant => self.append_answer(message_id, text),
            Chunk::User => self.append_user(message_id, text),
        }
    }

    /// Appends reasoning to the open bubble, opening one when needed.
    fn append_thought(&mut self, text: &str) {
        let index = self.open_assistant.unwrap_or_else(|| self.push_assistant());
        if text.is_empty() {
            return;
        }
        if let Item::Assistant {
            thinking,
            thinking_open,
            ..
        } = &mut self.items[index]
        {
            thinking.push_str(text);
            *thinking_open = true;
        }
    }

    /// Appends answer text, continuing the open bubble while it is still this
    /// message's to fill.
    fn append_answer(&mut self, message_id: &str, text: &str) {
        let continues = match self.open_assistant {
            Some(index) => match &self.items[index] {
                Item::Assistant { text: body, .. } => {
                    self.open_answer_id.as_deref() == Some(message_id) || body.is_empty()
                }
                _ => false,
            },
            None => false,
        };
        let index = if continues {
            self.open_assistant.expect("checked above")
        } else {
            self.push_assistant()
        };
        self.open_answer_id = Some(message_id.to_string());
        if text.is_empty() {
            return;
        }
        if let Item::Assistant { text: body, .. } = &mut self.items[index] {
            body.push_str(text);
        }
    }

    /// Appends an echoed user chunk, dropping an echo of a prompt the app
    /// already showed.
    ///
    /// The engine the app ships does not echo prompts, so the guard is a prefix
    /// test against the prompt the app just added: it costs nothing when no echo
    /// arrives and keeps the transcript from showing the user's own message
    /// twice if one ever does.
    fn append_user(&mut self, message_id: &str, text: &str) {
        if let Some(Item::User { id, text: prompt }) = self.items.last() {
            if id.starts_with("local-") && prompt.starts_with(text) {
                return;
            }
        }
        let index = match self.user_streams.get(message_id) {
            Some(&index) => index,
            None => {
                let index = self.items.len();
                self.items.push(Item::User {
                    id: format!("user-{message_id}"),
                    text: String::new(),
                });
                self.user_streams.insert(message_id.to_string(), index);
                index
            }
        };
        if text.is_empty() {
            return;
        }
        if let Item::User { text: body, .. } = &mut self.items[index] {
            body.push_str(text);
        }
    }

    /// Pushes a fresh assistant bubble and opens it for streaming.
    ///
    /// @returns its index in [`Conversation::items`]
    fn push_assistant(&mut self) -> usize {
        let index = self.items.len();
        self.items.push(Item::Assistant {
            id: format!("assistant-{index}"),
            text: String::new(),
            thinking: String::new(),
            thinking_open: false,
            rating: None,
        });
        self.open_assistant = Some(index);
        self.open_answer_id = None;
        index
    }

    /// Adds a tool card for a newly announced call.
    fn push_card(&mut self, call: &ToolCall) {
        if let Some(&index) = self.cards.get(&call.id) {
            // A repeated announcement is the same call; keep the card, take any
            // newer title or status.
            if let Item::Tool(card) = &mut self.items[index] {
                if !call.title.is_empty() {
                    card.title = call.title.clone();
                }
                card.status = ToolStatus::parse(&call.status);
                if call.input.is_some() {
                    card.input = call.input.clone();
                }
            }
            return;
        }
        let card = ToolCard {
            id: call.id.clone(),
            title: if call.title.is_empty() {
                "tool".to_string()
            } else {
                call.title.clone()
            },
            kind: call.kind.clone(),
            status: ToolStatus::parse(&call.status),
            input: call.input.clone(),
            output: String::new(),
            locations: Vec::new(),
            elapsed_ms: None,
        };
        let index = self.items.len();
        self.cards.insert(call.id.clone(), index);
        self.items.push(Item::Tool(card));
    }

    /// Merges one update into a tool card.
    fn update_card(
        &mut self,
        tool_call_id: &str,
        title: Option<&str>,
        status: Option<&str>,
        output: Option<&str>,
        input: Option<Value>,
        locations: &[String],
    ) {
        // An update for a call whose announcement was missed (a reattached
        // session) still deserves a card, so one is created.
        let index = match self.cards.get(tool_call_id) {
            Some(&index) => index,
            None => {
                self.push_card(&ToolCall {
                    id: tool_call_id.to_string(),
                    title: title.unwrap_or("tool").to_string(),
                    kind: "other".to_string(),
                    status: status.unwrap_or("in_progress").to_string(),
                    input: input.clone(),
                });
                *self
                    .cards
                    .get(tool_call_id)
                    .expect("the card was just pushed")
            }
        };
        let Item::Tool(card) = &mut self.items[index] else {
            return;
        };
        if let Some(title) = title {
            if !title.is_empty() {
                card.title = title.to_string();
            }
        }
        if let Some(input) = input {
            card.input = Some(input);
        }
        if let Some(status) = status {
            let next = ToolStatus::parse(status);
            if next.done() && !card.status.done() {
                card.elapsed_ms = self
                    .prompt_started
                    .map(|started| started.elapsed().as_millis() as u64);
            }
            card.status = next;
        }
        if let Some(output) = output {
            if !output.is_empty() {
                if !card.output.is_empty() && !card.output.ends_with('\n') {
                    card.output.push('\n');
                }
                card.output.push_str(output);
                self.meter.count_prompt(output);
            }
        }
        for location in locations {
            if !card.locations.contains(location) {
                card.locations.push(location.clone());
            }
        }
    }

    /// Marks every open thought as finished and closes the streaming bubble
    /// (the turn is over, so the next turn starts its own).
    fn close_bubbles(&mut self) {
        for item in &mut self.items {
            if let Item::Assistant { thinking_open, .. } = item {
                *thinking_open = false;
            }
        }
        self.open_assistant = None;
        self.open_answer_id = None;
        self.user_streams.clear();
    }

    /// The transcript as plain text, for copying or export.
    ///
    /// @returns one line per bubble or card, in order
    pub fn transcript_text(&self) -> String {
        let mut out = String::new();
        for item in &self.items {
            match item {
                Item::User { text, .. } => out.push_str(&format!("You:\n{text}\n\n")),
                Item::Assistant {
                    text,
                    thinking,
                    rating,
                    ..
                } => {
                    if !thinking.trim().is_empty() {
                        out.push_str(&format!("Thinking:\n{}\n\n", thinking.trim()));
                    }
                    if !text.trim().is_empty() {
                        out.push_str(&format!("Assistant:\n{}\n\n", text.trim()));
                    }
                    match rating {
                        Some(rating) if *rating > 0 => out.push_str("(rated useful)\n\n"),
                        Some(_) => out.push_str("(rated not useful)\n\n"),
                        None => {}
                    }
                }
                Item::Tool(card) => {
                    out.push_str(&format!(
                        "Tool {} [{}]:\n{}\n\n",
                        card.title,
                        card.status.label(),
                        card.output.trim()
                    ));
                }
                Item::Notice { text, .. } => out.push_str(&format!("· {text}\n\n")),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::{AgentEvent, AgentRequest, ConfigChoice, ConfigOption, ToolCall};

    /// A conversation with an open session, for reducer tests.
    fn session() -> Conversation {
        let mut conversation = Conversation::new(ConversationId(1));
        conversation.apply(&AgentEvent::SessionReady {
            session_id: "s-1".to_string(),
            config_options: vec![ConfigOption {
                id: "model".to_string(),
                name: "Model".to_string(),
                current_value: Some("[\"default\",\"deepseek-chat\"]".to_string()),
                choices: vec![ConfigChoice {
                    value: "[\"default\",\"deepseek-chat\"]".to_string(),
                    label: "deepseek-chat (mock)".to_string(),
                    group: Some("default".to_string()),
                }],
            }],
        });
        conversation
    }

    #[test]
    fn the_displayed_engine_identity_never_carries_the_vendored_dependency_s_own_name() {
        // The vendored engine's ACP handshake hardcodes its own package name
        // and an internal version it never bumps (`agentInfo: { name:
        // "deepseek-harness-acp", version: "0.0.1" }`); every UI surface that
        // shows `engine_name`/`engine_version` (the footer, the turn-failed
        // card, the observability panel, an exported transcript) must show
        // this application's own identity, not that string, however the wire
        // ever spells it.
        let mut conversation = Conversation::new(ConversationId(1));
        conversation.apply(&AgentEvent::Initialized {
            name: "deepseek-harness-acp".to_string(),
            version: "0.0.1".to_string(),
        });
        assert!(!conversation.engine_name.to_lowercase().contains("deepseek"));
        assert!(!conversation.engine_name.to_lowercase().contains("dsh"));
        assert_eq!(conversation.engine_name, "AI Harness engine");
        assert_eq!(conversation.engine_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn chunks_coalesce_into_one_bubble_and_keep_their_thoughts() {
        let mut conversation = session();
        conversation.begin_prompt("hello");
        for text in ["Wor", "king"] {
            conversation.apply(&AgentEvent::AgentMessage {
                session_id: "s-1".to_string(),
                message_id: "m-1".to_string(),
                text: text.to_string(),
            });
        }
        conversation.apply(&AgentEvent::AgentThought {
            session_id: "s-1".to_string(),
            message_id: "t-1".to_string(),
            text: "weighing options".to_string(),
        });
        conversation.apply(&AgentEvent::TurnEnded {
            session_id: "s-1".to_string(),
            stop_reason: "end_turn".to_string(),
        });

        let assistant: Vec<_> = conversation
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Assistant {
                    text,
                    thinking,
                    thinking_open,
                    ..
                } => Some((text.clone(), thinking.clone(), *thinking_open)),
                _ => None,
            })
            .collect();
        assert_eq!(assistant.len(), 1, "chunks must stream into one bubble");
        assert_eq!(assistant[0].0, "Working");
        assert_eq!(assistant[0].1, "weighing options");
        assert!(!assistant[0].2, "the thought closes when the turn ends");
        assert_eq!(conversation.state, RunState::Idle);
    }

    #[test]
    fn tool_cards_track_status_output_and_locations() {
        let mut conversation = session();
        conversation.begin_prompt("plan the refactor");
        conversation.apply(&AgentEvent::ToolCall {
            session_id: "s-1".to_string(),
            call: ToolCall {
                id: "call-1".to_string(),
                title: "wave_plan".to_string(),
                kind: "other".to_string(),
                status: "pending".to_string(),
                input: Some(serde_json::json!({ "subtasks": [] })),
            },
        });
        conversation.apply(&AgentEvent::ToolCallUpdate {
            session_id: "s-1".to_string(),
            tool_call_id: "call-1".to_string(),
            title: None,
            status: Some("completed".to_string()),
            output: Some("2 waves (depth 2):\n  wave 1 (parallel): store, ui".to_string()),
            input: None,
            locations: vec!["src/store.ts".to_string()],
        });

        let Item::Tool(card) = &conversation.items[1] else {
            panic!("expected a tool card")
        };
        assert_eq!(card.status, ToolStatus::Completed);
        assert!(card.output.contains("wave 1"));
        assert_eq!(card.locations, vec!["src/store.ts".to_string()]);
        assert!(card.elapsed_ms.is_some(), "completion records a duration");
        assert_eq!(card.summary(), "2 waves (depth 2):");
    }

    #[test]
    fn a_running_card_is_summarized_by_what_it_acts_on() {
        let card = ToolCard {
            id: "call-1".to_string(),
            title: "bash".to_string(),
            kind: "execute".to_string(),
            status: ToolStatus::Running,
            input: Some(
                serde_json::json!({ "command": "sleep 120", "description": "hold a process" }),
            ),
            output: String::new(),
            locations: Vec::new(),
            elapsed_ms: None,
        };
        assert_eq!(card.summary(), "sleep 120");

        let shaped = ToolCard {
            input: Some(serde_json::json!({ "limit": 20 })),
            ..card
        };
        assert_eq!(
            shaped.summary(),
            "1 field(s)",
            "with nothing to name, the shape is all there is"
        );
    }

    #[test]
    fn budget_escalations_are_recognizable_from_the_card() {
        let mut conversation = session();
        conversation.apply(&AgentEvent::ToolCall {
            session_id: "s-1".to_string(),
            call: ToolCall {
                id: "harness_budget".to_string(),
                title: "harness_budget".to_string(),
                kind: "other".to_string(),
                status: "pending".to_string(),
                input: None,
            },
        });
        let Item::Tool(card) = &conversation.items[0] else {
            panic!("expected a tool card")
        };
        assert!(card.is_budget_escalation());
    }

    #[test]
    fn an_unannounced_tool_update_still_gets_a_card() {
        let mut conversation = session();
        conversation.apply(&AgentEvent::ToolCallUpdate {
            session_id: "s-1".to_string(),
            tool_call_id: "call-orphan".to_string(),
            title: Some("read_file".to_string()),
            status: Some("completed".to_string()),
            output: Some("fn main() {}".to_string()),
            input: None,
            locations: Vec::new(),
        });
        assert_eq!(conversation.items.len(), 1);
        let Item::Tool(card) = &conversation.items[0] else {
            panic!("expected a tool card")
        };
        assert_eq!(card.title, "read_file");
        assert_eq!(card.status, ToolStatus::Completed);
    }

    #[test]
    fn usage_and_estimates_feed_the_meter() {
        let mut conversation = session();
        conversation.begin_prompt("please refactor the store module");
        conversation.apply(&AgentEvent::AgentMessage {
            session_id: "s-1".to_string(),
            message_id: "m-1".to_string(),
            text: "a".repeat(400),
        });
        conversation.apply(&AgentEvent::Usage {
            session_id: "s-1".to_string(),
            used: 7_076,
            size: 131_072,
        });
        conversation.apply(&AgentEvent::Usage {
            session_id: "s-1".to_string(),
            used: 7_900,
            size: 131_072,
        });

        assert_eq!(conversation.meter.used, 7_900);
        assert_eq!(conversation.meter.peak_used, 7_900);
        assert_eq!(conversation.meter.updates, 2);
        assert!((conversation.meter.context_fraction() - 0.0603).abs() < 0.001);
        assert!(conversation.meter.estimated_total() > 100);
        assert_eq!(
            conversation.meter.estimated_cost_usd(),
            None,
            "no prices, no cost claim"
        );

        conversation.meter.price = Some(Price {
            input: 1.0,
            output: 2.0,
            cache_read: None,
        });
        let cost = conversation
            .meter
            .estimated_cost_usd()
            .expect("price table set");
        assert!(cost > 0.0);
    }

    #[test]
    fn an_approval_request_marks_the_state_and_a_decision_lands_in_the_transcript() {
        let mut conversation = session();
        conversation.begin_prompt("amend the spec");
        conversation.apply(&AgentEvent::Request(AgentRequest::Permission {
            rpc_id: serde_json::json!(7),
            session_id: "s-1".to_string(),
            tool_call_id: Some("call-9".to_string()),
            title: Some("spec_amend".to_string()),
            reason: None,
            options: Vec::new(),
        }));
        assert_eq!(
            conversation.state,
            RunState::AwaitingApproval {
                title: "spec_amend".to_string()
            }
        );
        conversation.record_decision("spec_amend", Some("allow once"));
        assert_eq!(conversation.state, RunState::Running);
        assert!(matches!(
            conversation.items.last(),
            Some(Item::Notice {
                level: NoticeLevel::Info,
                ..
            })
        ));
    }

    #[test]
    fn a_failed_prompt_ends_the_run_instead_of_leaving_it_running() {
        let mut conversation = session();
        conversation.begin_prompt("do work");
        assert!(conversation.state.busy());
        conversation.apply(&AgentEvent::RequestFailed {
            method: "session/prompt".to_string(),
            message: "429: slow down".to_string(),
        });
        assert!(
            !conversation.state.busy(),
            "a turn that failed is over; a busy state here spins the UI forever"
        );
        assert_eq!(
            conversation.state,
            RunState::Failed {
                message: "429: slow down".to_string()
            }
        );
        assert!(conversation.items.iter().any(|item| matches!(
            item,
            Item::Notice {
                level: NoticeLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn a_failed_prompt_does_not_rewrite_a_failure_into_a_running_state() {
        // The engine can fail a later method while nothing is in flight; that is
        // a remark, not the end of a run, and it must not fail a finished
        // conversation.
        let mut conversation = session();
        conversation.apply(&AgentEvent::RequestFailed {
            method: "session/load".to_string(),
            message: "not supported".to_string(),
        });
        assert_eq!(conversation.state, RunState::Idle);
        assert!(conversation.items.iter().any(|item| matches!(
            item,
            Item::Notice {
                level: NoticeLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn engine_exit_fails_the_conversation_and_is_visible() {
        let mut conversation = session();
        conversation.begin_prompt("do work");
        conversation.apply(&AgentEvent::EngineExited { code: Some(1) });
        assert_eq!(
            conversation.state,
            RunState::Failed {
                message: "the agent engine exited with status 1".to_string()
            }
        );
        assert!(conversation.items.iter().any(|item| matches!(
            item,
            Item::Notice {
                level: NoticeLevel::Error,
                ..
            }
        )));
    }

    #[test]
    fn the_log_ring_stays_bounded_and_never_touches_the_transcript() {
        let mut conversation = session();
        for line in 0..LOG_LINES + 20 {
            conversation.apply(&AgentEvent::EngineLog {
                line: format!("line {line}"),
            });
        }
        assert_eq!(conversation.log.len(), LOG_LINES);
        assert_eq!(
            conversation.items.len(),
            0,
            "log lines are not transcript items"
        );
        assert_eq!(
            conversation.log.back().map(String::as_str),
            Some(format!("line {}", LOG_LINES + 19).as_str())
        );
    }

    #[test]
    fn unhandled_notifications_are_counted_rather_than_lost() {
        let mut conversation = session();
        for _ in 0..3 {
            conversation.apply(&AgentEvent::Unhandled {
                method: "session/something_new".to_string(),
            });
        }
        assert_eq!(
            conversation.unhandled.get("session/something_new"),
            Some(&3)
        );
    }

    #[test]
    fn transcript_export_keeps_the_order_of_bubbles_and_cards() {
        let mut conversation = session();
        conversation.begin_prompt("hi");
        conversation.apply(&AgentEvent::AgentMessage {
            session_id: "s-1".to_string(),
            message_id: "m-1".to_string(),
            text: "hello".to_string(),
        });
        conversation.apply(&AgentEvent::ToolCall {
            session_id: "s-1".to_string(),
            call: ToolCall {
                id: "call-1".to_string(),
                title: "bash".to_string(),
                kind: "execute".to_string(),
                status: "completed".to_string(),
                input: None,
            },
        });
        let text = conversation.transcript_text();
        let you = text.find("You:").expect("user line");
        let assistant = text.find("Assistant:").expect("assistant line");
        let tool = text.find("Tool bash").expect("tool line");
        assert!(you < assistant && assistant < tool);
    }

    #[test]
    fn the_active_model_comes_from_the_advertised_options() {
        let conversation = session();
        let active = conversation
            .active_model
            .expect("the current value is a selectable choice");
        assert_eq!(active.label, "deepseek-chat (mock)");
        assert_eq!(active.group.as_deref(), Some("default"));
    }

    /// A card with the given tool name and input, for the classification tests.
    fn card(title: &str, input: serde_json::Value) -> ToolCard {
        ToolCard {
            id: "call-1".to_string(),
            title: title.to_string(),
            kind: "other".to_string(),
            status: ToolStatus::Running,
            input: Some(input),
            output: String::new(),
            locations: Vec::new(),
            elapsed_ms: None,
        }
    }

    #[test]
    fn delegations_and_commands_are_recognised_by_name() {
        assert!(card("subagent", serde_json::json!({})).is_delegation());
        assert!(card("Subagent_Fork", serde_json::json!({})).is_delegation());
        assert!(card("committee", serde_json::json!({})).is_delegation());
        assert!(!card("bash", serde_json::json!({})).is_delegation());
        assert!(card("bash_persistent", serde_json::json!({})).is_command());
        assert!(!card("todo_write", serde_json::json!({})).is_command());
    }

    #[test]
    fn a_todo_call_carries_its_list() {
        let todos = card(
            "todo_write",
            serde_json::json!({ "todos": [
                { "content": "scan the repo", "status": "completed" },
                { "content": "write the fix", "status": "in_progress" },
                { "content": "run the tests" },
            ] }),
        );
        assert!(todos.is_todo_write());
        assert_eq!(
            todos.todos(),
            vec![
                ("scan the repo".to_string(), "completed".to_string()),
                ("write the fix".to_string(), "in_progress".to_string()),
                ("run the tests".to_string(), "pending".to_string()),
            ]
        );
        assert!(card("edit", serde_json::json!({ "file_path": "a.rs" }))
            .todos()
            .is_empty());
    }

    #[test]
    fn each_file_tool_spells_its_change_differently() {
        let edit = card(
            "edit",
            serde_json::json!({ "file_path": "src/a.rs", "old_string": "let x = 1;\nlet y = 2;", "new_string": "let x = 3;" }),
        );
        let change = edit.change().expect("edit changes a file");
        assert_eq!(change.path, "src/a.rs");
        assert_eq!(change.removed, vec!["let x = 1;", "let y = 2;"]);
        assert_eq!(change.added, vec!["let x = 3;"]);

        let write = card(
            "write",
            serde_json::json!({ "file_path": "notes.md", "content": "one\ntwo" }),
        );
        let change = write.change().expect("write changes a file");
        assert_eq!(change.removed.len(), 0);
        assert_eq!(change.added, vec!["one", "two"]);

        let replace = card(
            "str_replace_editor",
            serde_json::json!({ "command": "str_replace", "path": "/repo/a.py", "old_str": "a = 1", "new_str": "a = 2" }),
        );
        assert_eq!(
            replace.change().map(|change| change.added),
            Some(vec!["a = 2".to_string()])
        );
        let view = card(
            "str_replace_editor",
            serde_json::json!({ "command": "view", "path": "/repo/a.py" }),
        );
        assert!(view.change().is_none(), "view reads and changes nothing");
        assert!(card("read", serde_json::json!({ "file_path": "a.rs" }))
            .change()
            .is_none());
    }

    #[test]
    fn input_text_reads_the_first_field_that_holds_something() {
        let card = card(
            "subagent",
            serde_json::json!({ "description": "  ", "prompt": "do the thing" }),
        );
        assert_eq!(
            card.input_text(&["description", "prompt"]).as_deref(),
            Some("do the thing")
        );
        assert_eq!(card.input_text(&["missing"]), None);
    }

    #[test]
    fn a_rating_sticks_to_its_bubble_and_travels_in_the_export() {
        let mut conversation = session();
        conversation.begin_prompt("hello");
        conversation.apply(&AgentEvent::AgentMessage {
            session_id: "s-1".to_string(),
            message_id: "m-1".to_string(),
            text: "hi".to_string(),
        });
        let id = conversation
            .items
            .iter()
            .find_map(|item| match item {
                Item::Assistant { id, .. } => Some(id.clone()),
                _ => None,
            })
            .expect("a bubble streamed");
        assert!(conversation.rate(&id, Some(1)));
        assert!(
            !conversation.rate("assistant-99", Some(1)),
            "an unknown id rates nothing"
        );
        assert!(conversation.transcript_text().contains("(rated useful)"));
        assert!(conversation.rate(&id, None));
        assert!(!conversation.transcript_text().contains("(rated"));
    }
}
