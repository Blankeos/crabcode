use crate::llm::{ChunkMessage, ChunkSender};
use crate::tools::process_registry::{JobStatus, ProcessRegistry};
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub const DEFAULT_TERMINAL_ROWS: u16 = 24;
pub const DEFAULT_TERMINAL_COLS: u16 = 80;
pub const MAX_TRANSCRIPT_BYTES: usize = 51_200;
pub const MAX_MODEL_OUTPUT_BYTES: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionStart {
    pub session_id: String,
    pub tool_call_id: String,
    pub command: String,
    pub description: String,
    pub workdir: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// ProcessRegistry id when this session is tracked as an interactive job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TerminalSessionControl {
    Start {
        rows: u16,
        cols: u16,
    },
    Input(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    /// Complete the session using a terminal hosted by an external client.
    ExternalResult(TerminalSessionResult),
    /// Fail the session because an external terminal backend could not complete it.
    ExternalError(String),
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TerminalSessionEvent {
    Started,
    Output(Vec<u8>),
    Resized { rows: u16, cols: u16 },
    Exited { exit_code: Option<i32> },
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionResult {
    pub session_id: String,
    pub exit_code: Option<i32>,
    pub transcript_bytes: usize,
    pub transcript_truncated: bool,
    pub transcript_plain: String,
    pub cols: u16,
    pub rows: u16,
    pub stopped_by_user: bool,
}

/// UI opens an embedded terminal and drives the session via `control_tx`.
#[derive(Debug)]
pub struct TerminalSessionRequest {
    pub start: TerminalSessionStart,
    pub control_tx: mpsc::UnboundedSender<TerminalSessionControl>,
}

struct TranscriptState {
    raw: Vec<u8>,
    truncated: bool,
    cols: u16,
    rows: u16,
}

impl TranscriptState {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            raw: Vec::new(),
            truncated: false,
            cols,
            rows,
        }
    }

    fn append(&mut self, chunk: &[u8]) {
        if self.truncated {
            return;
        }
        let remaining = MAX_TRANSCRIPT_BYTES.saturating_sub(self.raw.len());
        if remaining == 0 {
            self.truncated = true;
            return;
        }
        let take = chunk.len().min(remaining);
        self.raw.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            self.truncated = true;
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    fn plain_text(&self) -> String {
        let text = sanitize_terminal_output(&self.raw);
        if text.len() <= MAX_MODEL_OUTPUT_BYTES {
            return text;
        }
        let end = text
            .char_indices()
            .nth(MAX_MODEL_OUTPUT_BYTES)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!(
            "{}\n\n[Transcript truncated to {} bytes]",
            &text[..end],
            MAX_MODEL_OUTPUT_BYTES
        )
    }
}

pub(crate) fn external_terminal_result(
    start: &TerminalSessionStart,
    output: &str,
    truncated: bool,
    exit_code: Option<i32>,
    stopped_by_user: bool,
) -> TerminalSessionResult {
    let mut transcript = TranscriptState::new(start.rows.max(1), start.cols.max(1));
    transcript.append(output.as_bytes());
    transcript.truncated |= truncated;
    TerminalSessionResult {
        session_id: start.session_id.clone(),
        exit_code,
        transcript_bytes: transcript.raw.len(),
        transcript_truncated: transcript.truncated,
        transcript_plain: transcript.plain_text(),
        cols: transcript.cols,
        rows: transcript.rows,
        stopped_by_user,
    }
}

/// Convert a PTY byte stream into display-safe text for chat history and model context.
/// The live terminal still receives the original bytes through the VT parser.
pub(crate) fn sanitize_terminal_output(raw: &[u8]) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        Ground,
        Escape,
        Csi,
        Osc,
        OscEscape,
        String,
        StringEscape,
    }

    fn erase_last_char(bytes: &mut Vec<u8>, line_start: usize) {
        if bytes.len() <= line_start {
            return;
        }
        let mut idx = bytes.len() - 1;
        while idx > line_start && bytes[idx] & 0b1100_0000 == 0b1000_0000 {
            idx -= 1;
        }
        bytes.truncate(idx);
    }

    let mut output = Vec::with_capacity(raw.len());
    let mut state = EscapeState::Ground;
    let mut line_start = 0usize;
    let mut pending_carriage_return = false;

    for &byte in raw {
        if pending_carriage_return {
            pending_carriage_return = false;
            if byte == b'\n' {
                output.push(b'\n');
                line_start = output.len();
                continue;
            }
            output.truncate(line_start);
        }

        match state {
            EscapeState::Ground => match byte {
                0x1b => state = EscapeState::Escape,
                b'\r' => pending_carriage_return = true,
                b'\n' => {
                    output.push(b'\n');
                    line_start = output.len();
                }
                b'\t' => output.extend_from_slice(b"    "),
                0x08 | 0x7f => erase_last_char(&mut output, line_start),
                0x00..=0x1f => {}
                _ => output.push(byte),
            },
            EscapeState::Escape => {
                state = match byte {
                    b'[' => EscapeState::Csi,
                    b']' => EscapeState::Osc,
                    b'P' | b'X' | b'^' | b'_' => EscapeState::String,
                    0x20..=0x2f => EscapeState::Escape,
                    _ => EscapeState::Ground,
                };
            }
            EscapeState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    state = EscapeState::Ground;
                }
            }
            EscapeState::Osc => match byte {
                0x07 => state = EscapeState::Ground,
                0x1b => state = EscapeState::OscEscape,
                _ => {}
            },
            EscapeState::OscEscape => {
                state = if byte == b'\\' {
                    EscapeState::Ground
                } else {
                    EscapeState::Osc
                };
            }
            EscapeState::String => {
                if byte == 0x1b {
                    state = EscapeState::StringEscape;
                }
            }
            EscapeState::StringEscape => {
                state = if byte == b'\\' {
                    EscapeState::Ground
                } else {
                    EscapeState::String
                };
            }
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn shell_command_builder(command: &str, workdir: Option<&PathBuf>) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("bash");
    cmd.arg("-c");
    cmd.arg(command);
    if let Some(dir) = workdir {
        cmd.cwd(dir);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd
}

fn emit_event(sender: &ChunkSender, tool_call_id: &str, event: TerminalSessionEvent) {
    let _ = sender.send(ChunkMessage::TerminalSessionEvent {
        tool_call_id: tool_call_id.to_string(),
        event,
    });
}

fn pty_exit_code(status: &portable_pty::ExitStatus) -> Option<i32> {
    let code = status.exit_code();
    if code > i32::MAX as u32 {
        None
    } else {
        Some(code as i32)
    }
}

fn kill_pty_child(child: &mut Box<dyn portable_pty::Child + Send + Sync>) {
    #[cfg(unix)]
    if let Some(pid) = child.process_id() {
        unsafe {
            let _ = libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

pub struct TerminalSessionTool {
    sender: Option<ChunkSender>,
    registry: Option<Arc<ProcessRegistry>>,
}

impl TerminalSessionTool {
    pub fn new() -> Self {
        Self {
            sender: None,
            registry: None,
        }
    }

    pub fn with_sender_opt(mut self, sender: Option<ChunkSender>) -> Self {
        self.sender = sender;
        self
    }

    pub fn with_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    async fn run_session(
        &self,
        sender: ChunkSender,
        mut start: TerminalSessionStart,
        control_rx: &mut mpsc::UnboundedReceiver<TerminalSessionControl>,
        ctx: &ToolContext,
    ) -> Result<TerminalSessionResult, ToolError> {
        let tool_call_id = start.tool_call_id.clone();
        let session_id = start.session_id.clone();
        let (rows, cols) = loop {
            if ctx.is_aborted() {
                emit_event(&sender, &tool_call_id, TerminalSessionEvent::Stopped);
                return Err(ToolError::Execution("Command aborted".to_string()));
            }
            tokio::select! {
                control = control_rx.recv() => match control {
                    Some(TerminalSessionControl::Start { rows, cols }) => {
                        break (rows.max(1), cols.max(1));
                    }
                    Some(TerminalSessionControl::Resize { rows, cols }) => {
                        start.rows = rows.max(1);
                        start.cols = cols.max(1);
                    }
                    Some(TerminalSessionControl::ExternalResult(result)) => return Ok(result),
                    Some(TerminalSessionControl::ExternalError(error)) => {
                        return Err(ToolError::Execution(error));
                    }
                    Some(TerminalSessionControl::Stop) | None => {
                        emit_event(&sender, &tool_call_id, TerminalSessionEvent::Stopped);
                        return Ok(TerminalSessionResult {
                            session_id,
                            exit_code: None,
                            transcript_bytes: 0,
                            transcript_truncated: false,
                            transcript_plain: String::new(),
                            cols: start.cols.max(1),
                            rows: start.rows.max(1),
                            stopped_by_user: true,
                        });
                    }
                    Some(TerminalSessionControl::Input(_)) => {}
                },
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        };
        start.rows = rows;
        start.cols = cols;
        let workdir = start.workdir.as_ref().map(PathBuf::from);

        let transcript = Arc::new(Mutex::new(TranscriptState::new(rows, cols)));
        let transcript_for_reader = transcript.clone();

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| ToolError::Execution(format!("Failed to open PTY: {}", e)))?;

        let cmd = shell_command_builder(&start.command, workdir.as_ref());
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| ToolError::Execution(format!("Failed to spawn PTY command: {}", e)))?;
        drop(pair.slave);

        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
        let master_for_resize = master.clone();

        let mut reader = master
            .lock()
            .map_err(|_| ToolError::Execution("PTY master lock poisoned".to_string()))?
            .try_clone_reader()
            .map_err(|e| ToolError::Execution(format!("Failed to clone PTY reader: {}", e)))?;

        let mut writer = master
            .lock()
            .map_err(|_| ToolError::Execution("PTY master lock poisoned".to_string()))?
            .take_writer()
            .map_err(|e| ToolError::Execution(format!("Failed to open PTY writer: {}", e)))?;

        let child = Arc::new(Mutex::new(child));
        let child_for_wait = child.clone();

        let (reader_done_tx, reader_done_rx) = oneshot::channel::<()>();
        let mut reader_done_rx = reader_done_rx;
        let output_sender = sender.clone();
        let output_tool_call_id = tool_call_id.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4_096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buffer[..n];
                        if let Ok(mut state) = transcript_for_reader.lock() {
                            state.append(chunk);
                        }
                        emit_event(
                            &output_sender,
                            &output_tool_call_id,
                            TerminalSessionEvent::Output(chunk.to_vec()),
                        );
                    }
                    Err(_) => break,
                }
            }
            let _ = reader_done_tx.send(());
        });

        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();
        let mut exit_rx = exit_rx;
        std::thread::spawn(move || {
            let mut exit_code = None;
            loop {
                let status = {
                    let mut guard = match child_for_wait.lock() {
                        Ok(guard) => guard,
                        Err(_) => break,
                    };
                    match guard.try_wait() {
                        Ok(Some(status)) => Some(pty_exit_code(&status)),
                        Ok(None) => None,
                        Err(_) => break,
                    }
                };
                if let Some(code) = status {
                    exit_code = code;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = exit_tx.send(exit_code);
        });

        emit_event(&sender, &tool_call_id, TerminalSessionEvent::Started);

        let mut stopped_by_user = false;
        let mut exit_code: Option<i32> = None;
        let mut reader_done = false;

        loop {
            if ctx.is_aborted() {
                stopped_by_user = true;
                if let Ok(mut guard) = child.lock() {
                    kill_pty_child(&mut guard);
                }
                break;
            }

            tokio::select! {
                control = control_rx.recv() => {
                    match control {
                        Some(TerminalSessionControl::Start { .. }) => {}
                        Some(TerminalSessionControl::Input(data)) => {
                            if !data.is_empty() {
                                let _ = writer.write_all(&data);
                                let _ = writer.flush();
                            }
                        }
                        Some(TerminalSessionControl::Resize { rows, cols }) => {
                            let rows = rows.max(1);
                            let cols = cols.max(1);
                            if let Ok(master) = master_for_resize.lock() {
                                let _ = master.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                            if let Ok(mut state) = transcript.lock() {
                                state.resize(rows, cols);
                            }
                            emit_event(
                                &sender,
                                &tool_call_id,
                                TerminalSessionEvent::Resized { rows, cols },
                            );
                        }
                        Some(TerminalSessionControl::ExternalResult(_)) => {}
                        Some(TerminalSessionControl::ExternalError(_)) => {}
                        Some(TerminalSessionControl::Stop) | None => {
                            stopped_by_user = true;
                            if let Ok(mut guard) = child.lock() {
                                kill_pty_child(&mut guard);
                            }
                            break;
                        }
                    }
                }
                code = &mut exit_rx => {
                    match code {
                        Ok(code) => {
                            exit_code = code;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                _ = &mut reader_done_rx, if !reader_done => {
                    reader_done = true;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        if stopped_by_user && exit_code.is_none() {
            if let Ok(Ok(code)) = tokio::time::timeout(Duration::from_secs(1), &mut exit_rx).await {
                exit_code = code;
            }
        }
        if !reader_done {
            let _ = tokio::time::timeout(Duration::from_millis(500), &mut reader_done_rx).await;
        }

        if stopped_by_user {
            emit_event(&sender, &tool_call_id, TerminalSessionEvent::Stopped);
        } else {
            emit_event(
                &sender,
                &tool_call_id,
                TerminalSessionEvent::Exited { exit_code },
            );
        }

        let (transcript_plain, transcript_bytes, transcript_truncated, final_cols, final_rows) = {
            let state = transcript
                .lock()
                .map_err(|_| ToolError::Execution("Transcript lock poisoned".to_string()))?;
            (
                state.plain_text(),
                state.raw.len(),
                state.truncated,
                state.cols,
                state.rows,
            )
        };

        Ok(TerminalSessionResult {
            session_id,
            exit_code,
            transcript_bytes,
            transcript_truncated,
            transcript_plain,
            cols: final_cols,
            rows: final_rows,
            stopped_by_user,
        })
    }
}

#[async_trait]
impl ToolHandler for TerminalSessionTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "terminal_session".to_string(),
            description:
                "Run an interactive shell command in a user-controlled embedded terminal. \
The user can type input, resize the terminal, and stop the session (ctrl+]); Esc minimizes \
(parks the session while keeping it running). Use this for interactive CLIs that need a TTY \
(prompts, pagers, curses). Prefer non-interactive `bash` when possible. For long-running \
commands you don't need to watch, use `bash` with mode=background and poll with bash_output / \
stop with bash_kill."
                    .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "command".to_string(),
                    description: "Shell command to run via `bash -c`".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "workdir".to_string(),
                    description: "Working directory for the command".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "description".to_string(),
                    description: "Human-readable description of what the command does".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["command"])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command = get_string_param(&params, "command")
            .ok_or_else(|| ToolError::Validation("command is required".to_string()))?;
        let workdir =
            get_string_param(&params, "workdir").or_else(|| get_string_param(&params, "path"));
        let description =
            get_string_param(&params, "description").unwrap_or_else(|| command.clone());

        let sender = self.sender.as_ref().ok_or_else(|| {
            ToolError::Execution("terminal_session tool has no chunk sender configured".to_string())
        })?;

        let session_id = cuid2::create_id();
        let tool_call_id = ctx.call_id.clone().unwrap_or_else(|| cuid2::create_id());

        let registry = self
            .registry
            .clone()
            .or_else(|| ctx.process_registry.clone());
        let workdir_path = workdir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.workdir().to_path_buf());
        // Prefer a job_id supplied by the caller (e.g. bash interactive already
        // registered). Otherwise register here so terminal_session jobs appear.
        let supplied_job_id = get_string_param(&params, "job_id");
        let job_id = if let Some(id) = supplied_job_id {
            Some(id)
        } else if let Some(ref registry) = registry {
            Some(
                registry
                    .register_interactive(command.clone(), description.clone(), &workdir_path)
                    .await,
            )
        } else {
            None
        };

        let (control_tx, mut control_rx) = mpsc::unbounded_channel();

        let start = TerminalSessionStart {
            session_id: session_id.clone(),
            tool_call_id: tool_call_id.clone(),
            command: command.clone(),
            description: description.clone(),
            workdir: workdir.clone(),
            cols: DEFAULT_TERMINAL_COLS,
            rows: DEFAULT_TERMINAL_ROWS,
            job_id: job_id.clone(),
        };

        if let Err(err) = sender.send(ChunkMessage::TerminalSessionRequest(
            TerminalSessionRequest {
                start: start.clone(),
                control_tx,
            },
        )) {
            if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
                registry
                    .mark_interactive_status(job_id, JobStatus::Failed, None)
                    .await;
            }
            return Err(ToolError::Execution(format!(
                "Failed to deliver terminal session request to UI: {err}"
            )));
        }

        if ctx.is_aborted() {
            if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
                registry
                    .mark_interactive_status(job_id, JobStatus::Killed, None)
                    .await;
            }
            return Err(ToolError::Execution("Cancelled".to_string()));
        }

        let result = match self
            .run_session(sender.clone(), start, &mut control_rx, ctx)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
                    registry
                        .mark_interactive_status(job_id, JobStatus::Failed, None)
                        .await;
                }
                return Err(err);
            }
        };

        if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
            let status = if result.stopped_by_user {
                JobStatus::Killed
            } else {
                JobStatus::Exited
            };
            registry
                .mark_interactive_status(job_id, status, result.exit_code)
                .await;
        }

        let output = if result.transcript_plain.trim().is_empty() {
            "(no output)".to_string()
        } else {
            result.transcript_plain.clone()
        };

        let exit_code = result.exit_code.unwrap_or(-1);
        let mut tool_result = ToolResult::new(format!("Terminal session: {}", description), output)
            .with_metadata("exit_code", serde_json::json!(exit_code))
            .with_metadata("command", serde_json::json!(command))
            .with_metadata("description", serde_json::json!(description))
            .with_metadata("workdir", serde_json::json!(workdir))
            .with_metadata("session_id", serde_json::json!(result.session_id))
            .with_metadata(
                "transcript_bytes",
                serde_json::json!(result.transcript_bytes),
            )
            .with_metadata(
                "transcript_truncated",
                serde_json::json!(result.transcript_truncated),
            )
            .with_metadata("cols", serde_json::json!(result.cols))
            .with_metadata("rows", serde_json::json!(result.rows))
            .with_metadata("stopped_by_user", serde_json::json!(result.stopped_by_user));
        if let Some(job_id) = job_id {
            tool_result = tool_result.with_metadata("task_id", serde_json::json!(job_id));
        }
        Ok(tool_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn transcript_caps_raw_bytes() {
        let mut state = TranscriptState::new(24, 80);
        let chunk = vec![b'x'; MAX_TRANSCRIPT_BYTES + 128];
        state.append(&chunk);
        assert_eq!(state.raw.len(), MAX_TRANSCRIPT_BYTES);
        assert!(state.truncated);
    }

    #[test]
    fn transcript_tracks_resize() {
        let mut state = TranscriptState::new(24, 80);
        state.resize(40, 120);
        assert_eq!(state.rows, 40);
        assert_eq!(state.cols, 120);
    }

    #[test]
    fn external_terminal_result_preserves_client_output_and_exit() {
        let start = TerminalSessionStart {
            session_id: "session".to_string(),
            tool_call_id: "call".to_string(),
            command: "echo hi".to_string(),
            description: "test".to_string(),
            workdir: None,
            cols: 80,
            rows: 24,
            job_id: None,
        };

        let result = external_terminal_result(&start, "\x1b[31mhi\x1b[0m\n", true, Some(7), false);

        assert_eq!(result.session_id, "session");
        assert_eq!(result.transcript_plain, "hi\n");
        assert!(result.transcript_truncated);
        assert_eq!(result.exit_code, Some(7));
        assert!(!result.stopped_by_user);
    }

    #[test]
    fn shell_command_builder_accepts_workdir() {
        let dir = PathBuf::from("/tmp/work");
        let _cmd = shell_command_builder("echo hi", Some(&dir));
    }

    #[test]
    fn terminal_output_is_safe_for_chat_rendering() {
        let raw = "\x1b]0;title\x07\x1b[2J\x1b[Hprogress 10%\rprogress 100%\r\nabc\x08XY\t\x1b[31mred\x1b[0m 🦀"
            .as_bytes();
        let text = sanitize_terminal_output(raw);

        assert_eq!(text, "progress 100%\nabXY    red 🦀");
        assert!(!text.chars().any(|ch| ch.is_control() && ch != '\n'));
    }

    #[tokio::test]
    async fn pty_prompt_accepts_user_input_and_completes() {
        let (sender, mut events) = mpsc::unbounded_channel();
        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        control_tx
            .send(TerminalSessionControl::Start { rows: 12, cols: 60 })
            .unwrap();
        control_tx
            .send(TerminalSessionControl::Input(b"Crab\r".to_vec()))
            .unwrap();

        let start = TerminalSessionStart {
            session_id: "test-session".to_string(),
            tool_call_id: "test-call".to_string(),
            command: "printf 'Name? '; IFS= read -r name; printf 'Hello %s\\n' \"$name\""
                .to_string(),
            description: "interactive test".to_string(),
            workdir: None,
            cols: 80,
            rows: 24,
            job_id: None,
        };
        let ctx =
            ToolContext::from_cancel_token("session", "message", "Build", CancellationToken::new());

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            TerminalSessionTool::new().run_session(sender, start, &mut control_rx, &ctx),
        )
        .await
        .expect("PTY command should finish")
        .expect("PTY command should succeed");

        assert_eq!(result.exit_code, Some(0));
        assert!(result.transcript_plain.contains("Name?"));
        assert!(result.transcript_plain.contains("Hello Crab"));
        assert!(events.try_recv().is_ok());
    }
}
