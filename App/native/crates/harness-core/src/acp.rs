//! Agent Client Protocol client: the app's only channel to the agent engine.
//!
//! The engine (`dsh` with our profile) is a child process speaking newline
//! delimited JSON-RPC 2.0 over stdin/stdout. This module owns that channel: it
//! spawns the engine, matches responses to requests, turns `session/update`
//! notifications into [`AgentEvent`]s, and answers the engine's own requests
//! (permission for a tool call) through [`AcpClient::respond_permission`].
//!
//! Three properties are deliberate:
//!
//! - **One writer thread.** Every outbound message goes through a channel to a
//!   single writer, so a request from the UI and a permission answer from the
//!   event loop can never interleave mid-line.
//! - **The reader never blocks the UI.** It parses and forwards; all state lives
//!   in the session model ([`crate::agent`]), which the UI owns.
//! - **Failure is loud.** A malformed line, an engine exit, or a request error is
//!   delivered as an event the UI shows, because a harness that silently stops
//!   answering is worse than one that says why.
//!
//! Method names, field names, and the update vocabulary come from the engine's
//! ACP bridge; the shapes are captured in [`AgentEvent`] so the rest of the app
//! never parses protocol JSON.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::EnginePaths;

/// A request the engine makes of the client, which the client must answer.
#[derive(Debug, Clone)]
pub enum AgentRequest {
    /// The engine wants a tool call approved before running it.
    Permission {
        /// JSON-RPC id to answer on.
        rpc_id: Value,
        /// Session the call belongs to.
        session_id: String,
        /// Tool call id, present when the asker named the call.
        tool_call_id: Option<String>,
        /// Title of the tool call, for the prompt.
        title: Option<String>,
        /// The engine's own reason, when it gave one.
        reason: Option<String>,
        /// Selectable outcomes, in the engine's order.
        options: Vec<PermissionOption>,
    },
}

/// One answer a permission request offers.
#[derive(Debug, Clone)]
pub struct PermissionOption {
    /// Opaque id echoed back when this option is chosen.
    pub option_id: String,
    /// Human label.
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, …
    pub kind: String,
}

/// Everything the engine reports to the app.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// The engine answered `initialize`.
    Initialized {
        /// Engine name from its agent info.
        name: String,
        /// Engine version.
        version: String,
    },
    /// A session was created (or attached).
    SessionReady {
        /// Session id.
        session_id: String,
        /// Selectable session configuration, as advertised.
        config_options: Vec<ConfigOption>,
    },
    /// A session's configuration options changed.
    ConfigOptions {
        /// Session id.
        session_id: String,
        /// The full new option list.
        config_options: Vec<ConfigOption>,
    },
    /// A chunk of assistant text.
    AgentMessage {
        /// Session id.
        session_id: String,
        /// Item id within the session, for streaming into one bubble.
        message_id: String,
        /// Text delta.
        text: String,
    },
    /// A chunk of the assistant's reasoning, when the route emits one.
    AgentThought {
        /// Session id.
        session_id: String,
        /// Item id within the session.
        message_id: String,
        /// Text delta.
        text: String,
    },
    /// The user's own message, echoed back for the transcript.
    UserMessage {
        /// Session id.
        session_id: String,
        /// Item id within the session.
        message_id: String,
        /// Text delta.
        text: String,
    },
    /// A tool call was announced.
    ToolCall {
        /// Session id.
        session_id: String,
        /// The call.
        call: ToolCall,
    },
    /// A tool call's state advanced (output, completion, failure).
    ToolCallUpdate {
        /// Session id.
        session_id: String,
        /// Call id being updated.
        tool_call_id: String,
        /// Updated title, when the engine supplied one.
        title: Option<String>,
        /// New status: `pending`, `in_progress`, `completed`, `failed`.
        status: Option<String>,
        /// Appended output text.
        output: Option<String>,
        /// Raw input, when the engine supplied it.
        input: Option<Value>,
        /// Files the call touched, for follow-along.
        locations: Vec<String>,
    },
    /// A plan (the model's own todo list) was published or updated.
    Plan {
        /// Session id.
        session_id: String,
        /// Entries in order.
        entries: Vec<PlanEntry>,
    },
    /// Token and context accounting for the session.
    Usage {
        /// Session id.
        session_id: String,
        /// Tokens currently in the context window.
        used: u64,
        /// The window's size.
        size: u64,
    },
    /// The turn ended.
    TurnEnded {
        /// Session id.
        session_id: String,
        /// Why: `end_turn`, `max_tokens`, `refusal`, `cancelled`, …
        stop_reason: String,
    },
    /// The engine asks the client to decide something.
    Request(AgentRequest),
    /// A JSON-RPC error arrived for a request the app made.
    RequestFailed {
        /// Method that failed.
        method: String,
        /// The engine's message.
        message: String,
    },
    /// A notification or response the app does not model yet, kept for diagnostics.
    Unhandled {
        /// The raw method name, when it had one.
        method: String,
    },
    /// The engine's stderr produced a line.
    EngineLog {
        /// The line, verbatim.
        line: String,
    },
    /// The engine process ended.
    EngineExited {
        /// Exit code, when it exited normally.
        code: Option<i32>,
    },
}

/// One session configuration option the engine advertises.
#[derive(Debug, Clone)]
pub struct ConfigOption {
    /// Option id, e.g. `model`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Current opaque value.
    pub current_value: Option<String>,
    /// Selectable values.
    pub choices: Vec<ConfigChoice>,
}

/// One selectable value of a configuration option.
#[derive(Debug, Clone)]
pub struct ConfigChoice {
    /// Opaque value to select.
    pub value: String,
    /// Display label.
    pub label: String,
    /// Optional grouping label (the engine groups models by route).
    pub group: Option<String>,
}

/// A tool call as the engine reports it.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Protocol call id.
    pub id: String,
    /// Title shown on the card.
    pub title: String,
    /// Category: `read`, `edit`, `execute`, `search`, `fetch`, `other`, …
    pub kind: String,
    /// Status at announcement.
    pub status: String,
    /// Raw input, when supplied.
    pub input: Option<Value>,
}

/// One entry of the model's plan.
#[derive(Debug, Clone)]
pub struct PlanEntry {
    /// What the entry is about.
    pub content: String,
    /// `high`, `medium`, `low`.
    pub priority: String,
    /// `pending`, `in_progress`, `completed`.
    pub status: String,
}

/// Handle to one running engine process.
///
/// Dropping the handle kills the engine: an app that has gone away must not
/// leave an agent spending tokens in the background.
pub struct AcpClient {
    child: Mutex<Option<Child>>,
    outbound: Sender<String>,
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, String>>>,
}

impl AcpClient {
    /// Spawns the engine and starts its reader and writer threads.
    ///
    /// The engine's own working directory is the repo root, not the workspace:
    /// that is what puts the repo's `.env` in its project env layer. The
    /// workspace the agent operates on travels in `session/new` and, so the
    /// launcher can read that workspace's route document, in
    /// `HARNESS_WORKSPACE`.
    ///
    /// @param paths resolved engine layout
    /// @param events sink for every event the engine produces
    /// @param workspace the workspace this engine is being started for
    /// @returns the client handle
    /// @throws `Err` when the engine cannot be spawned or its pipes are unavailable
    pub fn spawn(
        paths: &EnginePaths,
        events: Sender<AgentEvent>,
        workspace: &std::path::Path,
    ) -> Result<Self, String> {
        let (program, args) = paths.engine_command();
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(&paths.repo_root)
            .env("DSH_HOME", &paths.dsh_home)
            .env("HARNESS_WORKSPACE", workspace)
            .env("TSX_TSCONFIG_PATH", paths.dsh_repo.join("tsconfig.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|err| {
            format!(
                "cannot start the harness engine at {}: {err}",
                program.display()
            )
        })?;
        let stdin: ChildStdin = child.stdin.take().ok_or("engine stdin was not captured")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("engine stdout was not captured")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("engine stderr was not captured")?;

        // One writer thread owns stdin, so concurrent senders cannot interleave
        // a line, and a closed pipe reports itself once instead of panicking in
        // whichever thread noticed.
        let (outbound, outbound_rx): (Sender<String>, Receiver<String>) = channel();
        thread::Builder::new()
            .name("acp-writer".into())
            .spawn(move || {
                let mut stdin = stdin;
                while let Ok(line) = outbound_rx.recv() {
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err()
                    {
                        break;
                    }
                    if stdin.flush().is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| format!("cannot start the ACP writer thread: {err}"))?;

        let pending: Arc<Mutex<HashMap<i64, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let failed = pending.clone();
        let reader_events = events.clone();
        thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    dispatch_line(&line, &reader_events, &failed);
                }
            })
            .map_err(|err| format!("cannot start the ACP reader thread: {err}"))?;

        let log_events = events;
        thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let _ = log_events.send(AgentEvent::EngineLog { line });
                }
            })
            .map_err(|err| format!("cannot start the engine log thread: {err}"))?;

        Ok(Self {
            child: Mutex::new(Some(child)),
            outbound,
            next_id: AtomicI64::new(1),
            pending,
        })
    }

    /// Sends one JSON-RPC request and returns its id.
    ///
    /// The response arrives as events, not as a return value: the UI must stay
    /// responsive while a prompt runs for minutes, so nothing here blocks on the
    /// engine.
    ///
    /// @param method ACP method name
    /// @param params JSON parameters
    /// @returns the request id
    pub fn request(&self, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending
            .lock()
            .expect("pending map")
            .insert(id, method.to_string());
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Answers a request the engine made.
    ///
    /// @param rpc_id the request's id, as delivered in [`AgentRequest`]
    /// @param result JSON result
    pub fn respond(&self, rpc_id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": rpc_id, "result": result }));
    }

    /// Answers a permission request by choosing one of its options.
    ///
    /// @param rpc_id the request's id
    /// @param option_id the chosen option, or `None` to cancel
    pub fn respond_permission(&self, rpc_id: &Value, option_id: Option<&str>) {
        let result = match option_id {
            Some(option) => json!({ "outcome": { "outcome": "selected", "optionId": option } }),
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        self.respond(rpc_id, result);
    }

    /// `initialize` — the first call, once per engine.
    ///
    /// @returns the request id
    pub fn initialize(&self) -> i64 {
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
                "clientInfo": { "name": "ai-harness", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
    }

    /// `session/new` — opens a session rooted at a workspace.
    ///
    /// @param workspace session working directory
    /// @returns the request id
    pub fn new_session(&self, workspace: &str) -> i64 {
        self.request("session/new", json!({ "cwd": workspace, "mcpServers": [] }))
    }

    /// `session/prompt` — sends a user turn.
    ///
    /// @param session_id session to prompt
    /// @param text the user's message
    /// @returns the request id
    pub fn prompt(&self, session_id: &str, text: &str) -> i64 {
        self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }],
            }),
        )
    }

    /// `session/cancel` — asks the engine to stop the running turn.
    ///
    /// @param session_id session to cancel
    /// @returns the request id
    pub fn cancel(&self, session_id: &str) -> i64 {
        self.request("session/cancel", json!({ "sessionId": session_id }))
    }

    /// `session/set_config_option` — selects a model (or another advertised option).
    ///
    /// @param session_id session to configure
    /// @param config_id option id, e.g. `model`
    /// @param value opaque value returned by a previous option state
    /// @returns the request id
    pub fn set_config_option(&self, session_id: &str, config_id: &str, value: &str) -> i64 {
        self.request(
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": config_id, "value": value }),
        )
    }

    /// Queues one already-encoded JSON line.
    fn send(&self, message: Value) {
        let _ = self.outbound.send(message.to_string());
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        // Closing stdin is the engine's shutdown signal (the ACP app binds EOF
        // to a bounded exit), so the client asks first and only kills a process
        // that ignored the request.
        let (closed, _) = channel();
        drop(std::mem::replace(&mut self.outbound, closed));
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                if !wait_briefly(child, Duration::from_secs(5)) {
                    log::warn!("acp: engine ignored stdin EOF; killing it");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

/// Waits for a child to exit, up to a bound.
///
/// @param child process to reap
/// @param bound how long to wait
/// @returns whether the child exited within the bound
fn wait_briefly(child: &mut Child, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => return false,
            // A child that cannot be waited on is not going to be killed
            // gracefully; report it as still running so the caller escalates.
            Err(_) => return false,
        }
    }
}

/// Routes one line from the engine: a response, a request, or a notification.
fn dispatch_line(
    line: &str,
    events: &Sender<AgentEvent>,
    pending: &Arc<Mutex<HashMap<i64, String>>>,
) {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        log::warn!("acp: unparseable line from the engine: {line}");
        return;
    };
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    match (method, id) {
        // A request from the engine that expects an answer.
        (Some(method), Some(id)) => {
            // A method *and* a result/error is neither a request nor a response;
            // answering it would be guesswork, so it is reported and dropped.
            if message.get("result").is_some() || message.get("error").is_some() {
                log::warn!("acp: message from the engine is neither request nor response: {line}");
                return;
            }
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "session/request_permission" => {
                    let request = permission_request(id, &params);
                    let _ = events.send(AgentEvent::Request(request));
                }
                // Unknown client-side requests are declined rather than ignored:
                // an engine waiting forever on a client is a hang.
                other => {
                    let _ = events.send(AgentEvent::Unhandled {
                        method: other.to_string(),
                    });
                    let _ = events.send(AgentEvent::RequestFailed {
                        method: other.to_string(),
                        message: "the app does not implement this client request".to_string(),
                    });
                }
            }
        }
        // A response to something the app asked for.
        (None, Some(id)) => {
            let method = id
                .as_i64()
                .and_then(|id| pending.lock().ok().and_then(|mut map| map.remove(&id)))
                .unwrap_or_else(|| "?".to_string());
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                let _ = events.send(AgentEvent::RequestFailed {
                    method,
                    message: text,
                });
                return;
            }
            let result = message.get("result").cloned().unwrap_or(Value::Null);
            match method.as_str() {
                "initialize" => {
                    let name = result
                        .pointer("/agentInfo/name")
                        .and_then(Value::as_str)
                        .unwrap_or("agent");
                    let version = result
                        .pointer("/agentInfo/version")
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    let _ = events.send(AgentEvent::Initialized {
                        name: name.to_string(),
                        version: version.to_string(),
                    });
                }
                "session/new" => {
                    let session_id = result
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let config_options = config_options(result.get("configOptions"));
                    let _ = events.send(AgentEvent::SessionReady {
                        session_id: session_id.to_string(),
                        config_options,
                    });
                }
                "session/set_config_option" => {
                    let config_options = config_options(Some(&result));
                    let _ = events.send(AgentEvent::ConfigOptions {
                        session_id: String::new(),
                        config_options,
                    });
                }
                "session/prompt" => {
                    let stop_reason = result
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or("end_turn")
                        .to_string();
                    let _ = events.send(AgentEvent::TurnEnded {
                        session_id: String::new(),
                        stop_reason,
                    });
                }
                _ => {
                    let _ = events.send(AgentEvent::Unhandled { method });
                }
            }
        }
        // A notification: the session update stream.
        (Some("session/update"), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(update) = params.get("update") {
                dispatch_update(&session_id, update, events);
            }
        }
        (Some(other), None) => {
            let _ = events.send(AgentEvent::Unhandled {
                method: other.to_string(),
            });
        }
        // No method and no id: an engine notification shape the app does not know.
        (None, None) => {
            if message.get("result").is_none() && message.get("error").is_none() {
                log::warn!("acp: unrecognized message from the engine: {line}");
            }
        }
    }
}

/// Decodes one `session/update` payload.
fn dispatch_update(session_id: &str, update: &Value, events: &Sender<AgentEvent>) {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text_of = |value: &Value| -> String { content_text(value) };
    match kind {
        "agent_message_chunk" => {
            let _ = events.send(AgentEvent::AgentMessage {
                session_id: session_id.to_string(),
                message_id: update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                text: text_of(update.get("content").unwrap_or(&Value::Null)),
            });
        }
        "agent_thought_chunk" => {
            let _ = events.send(AgentEvent::AgentThought {
                session_id: session_id.to_string(),
                message_id: update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                text: text_of(update.get("content").unwrap_or(&Value::Null)),
            });
        }
        "user_message_chunk" => {
            let _ = events.send(AgentEvent::UserMessage {
                session_id: session_id.to_string(),
                message_id: update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                text: text_of(update.get("content").unwrap_or(&Value::Null)),
            });
        }
        "tool_call" => {
            let _ = events.send(AgentEvent::ToolCall {
                session_id: session_id.to_string(),
                call: ToolCall {
                    id: update
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    title: update
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    kind: update
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("other")
                        .to_string(),
                    status: update
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .to_string(),
                    input: update.get("rawInput").cloned(),
                },
            });
        }
        "tool_call_update" => {
            let _ = events.send(AgentEvent::ToolCallUpdate {
                session_id: session_id.to_string(),
                tool_call_id: update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                title: update
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                status: update
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                output: update.get("content").map(|content| {
                    content
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .map(content_text)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                }),
                input: update.get("rawInput").cloned(),
                locations: update
                    .get("locations")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                item.get("path").and_then(Value::as_str).map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        "plan" => {
            let entries = update
                .get("entries")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| PlanEntry {
                            content: item
                                .get("content")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            priority: item
                                .get("priority")
                                .and_then(Value::as_str)
                                .unwrap_or("medium")
                                .to_string(),
                            status: item
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("pending")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = events.send(AgentEvent::Plan {
                session_id: session_id.to_string(),
                entries,
            });
        }
        "usage_update" => {
            let _ = events.send(AgentEvent::Usage {
                session_id: session_id.to_string(),
                used: update.get("used").and_then(Value::as_u64).unwrap_or(0),
                size: update.get("size").and_then(Value::as_u64).unwrap_or(0),
            });
        }
        other => {
            let _ = events.send(AgentEvent::Unhandled {
                method: other.to_string(),
            });
        }
    }
}

/// Plain text of one ACP content block (`{"type":"text","text":…}` and friends).
fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
            // A tool-call update's content wraps the block one level deeper.
            if let Some(inner) = map.get("content") {
                return content_text(inner);
            }
            String::new()
        }
        Value::Array(items) => items.iter().map(content_text).collect::<Vec<_>>().join(""),
        _ => String::new(),
    }
}

/// Decodes the `session/request_permission` params.
fn permission_request(rpc_id: Value, params: &Value) -> AgentRequest {
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let option_id = item.get("optionId").and_then(Value::as_str)?;
                    Some(PermissionOption {
                        option_id: option_id.to_string(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(option_id)
                            .to_string(),
                        kind: item
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("allow_once")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    AgentRequest::Permission {
        rpc_id,
        session_id: params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tool_call_id: params
            .pointer("/toolCall/toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: params
            .pointer("/toolCall/title")
            .and_then(Value::as_str)
            .map(str::to_string),
        reason: params
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        options,
    }
}

/// Decodes the ACP session config-option list, which may be a bare array or the
/// engine's `configOptions` key.
fn config_options(value: Option<&Value>) -> Vec<ConfigOption> {
    let Some(value) = value else {
        return Vec::new();
    };
    let array = match value {
        Value::Array(items) => items.clone(),
        Value::Object(map) => match map.get("configOptions") {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    array
        .iter()
        .filter_map(|option| {
            let id = option.get("id").and_then(Value::as_str)?.to_string();
            let choices = option
                .get("options")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .flat_map(|item| match item.get("group").and_then(Value::as_str) {
                            // A grouped option lists its values one level down.
                            Some(group) => item
                                .get("options")
                                .and_then(Value::as_array)
                                .map(|inner| {
                                    inner
                                        .iter()
                                        .filter_map(|leaf| {
                                            Some(ConfigChoice {
                                                value: leaf
                                                    .get("value")
                                                    .and_then(Value::as_str)?
                                                    .to_string(),
                                                label: leaf
                                                    .get("name")
                                                    .and_then(Value::as_str)?
                                                    .to_string(),
                                                group: Some(group.to_string()),
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default(),
                            None => {
                                let value = item.get("value").and_then(Value::as_str);
                                let label = item.get("name").and_then(Value::as_str);
                                match (value, label) {
                                    (Some(value), Some(label)) => vec![ConfigChoice {
                                        value: value.to_string(),
                                        label: label.to_string(),
                                        group: None,
                                    }],
                                    _ => Vec::new(),
                                }
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ConfigOption {
                id,
                name: option
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("option")
                    .to_string(),
                current_value: option
                    .get("currentValue")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                choices,
            })
        })
        .collect()
}
