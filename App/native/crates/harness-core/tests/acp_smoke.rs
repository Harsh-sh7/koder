//! End-to-end proof of the Rust ACP client against the real engine.
//!
//! Opt-in, because it needs a prepared checkout (`make setup`) and the mock
//! model route the repository's `.harness/models.json` points at. It is the
//! Rust half of the handshake `App/probe/acp-smoke.mjs` proves from Node:
//! the engine boots, a session opens, a prompt runs, the harness's own
//! orchestration tool reports its card, and the turn ends.
//!
//! Run with:
//! `cargo test -p harness-core --test acp_smoke -- --ignored --nocapture`

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use harness_core::acp::{AcpClient, AgentEvent, AgentRequest};
use harness_core::agent::{Conversation, Item, RunState, ToolStatus};
use harness_core::config::EnginePaths;
use harness_core::event::ConversationId;

/// How long any single step of the handshake may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);
/// The port `<workspace>/.harness/models.json` points the route at.
const MOCK_PORT: u16 = 8899;

/// A running mock model server, killed when the test ends.
struct MockServer {
    child: Child,
}

impl MockServer {
    /// Starts `App/probe/mock-openai.mjs --wave` and waits for its port.
    ///
    /// @param repo_root repository root
    /// @param node supported interpreter, as resolved with the engine paths
    /// @throws `Err` naming the failure when the server cannot start
    fn start(repo_root: &Path, node: &Path) -> Result<Self, String> {
        // The mock's behaviour is a state machine over the process's lifetime,
        // so talking to somebody else's instance silently tests the wrong
        // thing. A busy port is refused rather than joined.
        if TcpStream::connect(("127.0.0.1", MOCK_PORT)).is_ok() {
            return Err(format!(
                "port {MOCK_PORT} is already serving; stop that process before running this test"
            ));
        }
        let script = repo_root.join("App").join("probe").join("mock-openai.mjs");
        let child = Command::new(node)
            .arg(&script)
            .arg("--wave")
            .arg("--port")
            .arg(MOCK_PORT.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("cannot start the mock model server: {err}"))?;
        let server = Self { child };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", MOCK_PORT)).is_ok() {
                return Ok(server);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "the mock model server never accepted a connection on port {MOCK_PORT}"
        ))
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The client plus the event stream, with the engine's requests answered and
/// every event reduced into the session model the panes draw from.
struct Session {
    client: AcpClient,
    events: Receiver<AgentEvent>,
    seen: Vec<AgentEvent>,
    conversation: Conversation,
}

impl Session {
    /// Waits for the next event, answering permission requests the way the app
    /// does: grant once. Everything seen is kept for the assertions.
    ///
    /// @param timeout how long to wait
    /// @throws `Err` on timeout, a closed channel, or an engine failure
    fn next_event(&mut self, timeout: Duration) -> Result<AgentEvent, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(remaining) {
                Ok(AgentEvent::Request(AgentRequest::Permission { rpc_id, title, .. })) => {
                    self.conversation.record_decision(
                        title.as_deref().unwrap_or("a tool call"),
                        Some("allow once"),
                    );
                    self.client.respond_permission(&rpc_id, Some("allow-once"));
                }
                Ok(event) => {
                    self.conversation.apply(&event);
                    self.seen.push(event.clone());
                    return Ok(event);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err("timed out waiting for the engine".to_string())
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the engine event channel closed".to_string())
                }
            }
        }
    }

    /// Waits for a specific event, reporting anything else it passes.
    ///
    /// @param label what is being awaited, for the failure message
    /// @param pick extracts the payload, or `None` to keep waiting
    /// @throws `Err` when the wait ends without the event
    fn wait_for<T>(
        &mut self,
        label: &str,
        timeout: Duration,
        pick: impl Fn(&AgentEvent) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = self
                .next_event(remaining)
                .map_err(|err| format!("{label}: {err}"))?;
            match pick(&event) {
                Some(value) => return Ok(value),
                None => match &event {
                    AgentEvent::RequestFailed { method, message } => {
                        return Err(format!("{label}: {method} failed: {message}"));
                    }
                    AgentEvent::EngineExited { code } => {
                        return Err(format!("{label}: the engine exited with {code:?}"));
                    }
                    other => eprintln!("  (while awaiting {label}) {}", describe(other)),
                },
            }
        }
    }
}

/// One line describing an event, for the test log.
fn describe(event: &AgentEvent) -> String {
    match event {
        AgentEvent::AgentMessage { text, .. } => format!("message {:?}", truncate(text)),
        AgentEvent::AgentThought { text, .. } => format!("thought {:?}", truncate(text)),
        AgentEvent::ToolCall { call, .. } => format!("tool_call {} ({})", call.title, call.kind),
        AgentEvent::ToolCallUpdate {
            tool_call_id,
            status,
            output,
            ..
        } => format!(
            "tool_call_update {tool_call_id} {} {}",
            status.as_deref().unwrap_or("-"),
            output.as_deref().map(truncate).unwrap_or_default()
        ),
        AgentEvent::Plan { entries, .. } => format!("plan ({} entries)", entries.len()),
        AgentEvent::Usage { used, size, .. } => format!("usage {used}/{size}"),
        AgentEvent::EngineLog { line } => format!("engine: {}", truncate(line)),
        other => format!("{other:?}"),
    }
}

/// Keeps a description short enough to read in test output.
fn truncate(text: &str) -> String {
    let mut out: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        out.push('…');
    }
    out
}

#[test]
#[ignore = "requires `make setup`, node, and the mock route in .harness/models.json"]
fn the_rust_client_boots_the_engine_and_receives_a_wave_plan() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate lives at <repo>/App/native/crates/harness-core")
        .to_path_buf();
    let paths = EnginePaths::resolve(&workspace, None).expect("engine layout");
    let _mock = MockServer::start(&paths.repo_root, &paths.node).expect("mock model server");

    let (tx, rx) = channel();
    let client = AcpClient::spawn(&paths, tx, &workspace).expect("engine spawn");
    let mut session = Session {
        client,
        events: rx,
        seen: Vec::new(),
        conversation: Conversation::new(ConversationId(1)),
    };

    session.client.initialize();
    let (name, version) = session
        .wait_for("initialize", STEP_TIMEOUT, |event| match event {
            AgentEvent::Initialized { name, version } => Some((name.clone(), version.clone())),
            _ => None,
        })
        .expect("initialize response");
    eprintln!("initialized: {name} {version}");

    let workspace_arg = workspace.to_string_lossy().into_owned();
    session.client.new_session(&workspace_arg);
    let session_id = session
        .wait_for("session/new", STEP_TIMEOUT, |event| match event {
            AgentEvent::SessionReady {
                session_id,
                config_options,
            } => Some((session_id.clone(), config_options.len())),
            _ => None,
        })
        .expect("session/new response");
    eprintln!(
        "session {} with {} config option(s)",
        session_id.0, session_id.1
    );
    let session_id = session_id.0;

    let prompt = "Plan the refactor of the store module.";
    session.conversation.begin_prompt(prompt);
    session.client.prompt(&session_id, prompt);
    let stop_reason = session
        .wait_for("session/prompt", STEP_TIMEOUT, |event| match event {
            AgentEvent::TurnEnded { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .expect("turn end");
    eprintln!("turn ended: {stop_reason}");
    assert_eq!(
        stop_reason, "end_turn",
        "the mock model ends its turn normally"
    );

    // The prompt's point: the engine ran the harness's own orchestration tool,
    // and the session model shows it as a card. This is the vocabulary the
    // agent pane renders, proven against the real engine rather than a fixture.
    let conversation = &session.conversation;
    eprintln!("transcript:\n{}", conversation.transcript_text());

    let wave = conversation
        .items
        .iter()
        .find_map(|item| match item {
            Item::Tool(card) if card.title.contains("wave_plan") => Some(card),
            _ => None,
        })
        .expect("the wave_plan card must be in the transcript");
    eprintln!(
        "wave card: {} [{}] {}",
        wave.title,
        wave.status.label(),
        wave.summary()
    );
    assert_eq!(wave.status, ToolStatus::Completed, "the wave plan finished");
    assert!(
        wave.output.contains("wave 1") && wave.output.contains("wave 2"),
        "the wave plan reports its waves, got {:?}",
        wave.output
    );
    assert!(
        wave.elapsed_ms.is_some(),
        "a completed card records how long it ran"
    );

    assert_eq!(conversation.state, RunState::Idle, "the turn is over");
    assert!(
        conversation.meter.used > 0,
        "the engine reported context usage"
    );
    assert!(
        conversation.meter.size > conversation.meter.used,
        "the context window is larger than its occupancy"
    );
    assert!(
        conversation
            .items
            .iter()
            .any(|item| matches!(item, Item::Assistant { text, .. } if !text.trim().is_empty())),
        "the model's answer is in the transcript"
    );
    eprintln!(
        "context: {} / {} ({:.1}%), estimated session tokens {}",
        conversation.meter.used,
        conversation.meter.size,
        conversation.meter.context_fraction() * 100.0,
        conversation.meter.estimated_total()
    );
}
