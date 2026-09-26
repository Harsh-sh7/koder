//! Event vocabulary shared between the core and the UI.
//!
//! The core runs background subsystems (PTY readers, ACP notifications, git
//! watchers). Each publishes [`CoreEvent`] values into one crossbeam channel
//! that the UI drains every frame. This keeps the UI a pure function of
//! drained events plus its own query calls.

use std::path::PathBuf;

/// Identifies a terminal session owned by the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TerminalId(pub u64);

/// Identifies an agent conversation owned by the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ConversationId(pub u64);

/// Events published by core subsystems.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// A terminal's screen content advanced; the UI should repaint that pane.
    TerminalUpdated(TerminalId),
    /// A terminal's child process exited.
    TerminalExited { id: TerminalId, code: Option<i32> },
    /// A block (command + output) finished with an exit status.
    BlockCompleted {
        id: TerminalId,
        exit_code: i32,
        command: String,
    },
    /// Agent engine produced a new conversation item.
    AgentUpdated(ConversationId),
    /// Agent engine run state changed (idle, running, waiting for approval).
    AgentStateChanged(ConversationId),
    /// The child agent engine exited unexpectedly.
    EngineExited { code: Option<i32> },
    /// Files changed on disk under a watched workspace root.
    WorkspaceChanged(PathBuf),
    /// Git state changed under a watched repository root.
    GitStateChanged(PathBuf),
}

/// Convenience alias for the core's event sink.
pub type EventSink = crossbeam_channel::Sender<CoreEvent>;
/// Convenience alias for the UI's event source.
pub type EventStream = crossbeam_channel::Receiver<CoreEvent>;
