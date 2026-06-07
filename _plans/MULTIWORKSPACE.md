# Multi-Workspace Sessions Plan

## Goal

Make `crabcode` work more like a multi-chat agent TUI by default. A terminal run of `crabcode` should behave like a client tab into a shared set of sessions, not like an isolated one-off process.

The important shift is this:

- A **workspace** is a folder/project root.
- A **session** is a chat thread inside a workspace.
- A **client** is one running TUI instance.
- A **generation** is one assistant/agent turn that may be streaming, using tools, waiting for permission, completed, failed, or cancelled.
- A **runtime** is the process layer that owns active generations so they can keep running after a TUI exits.

This is intentionally not a worktree feature for now. Multiple sessions can exist for the same folder, but they share the same filesystem checkout.

The user-facing name should be **multiworkspace**, following Zed's wording.

## Desired Product Shape

`crabcode` should feel like a terminal-native chat app:

- Start in the current workspace, with a current session selected or ready to create one.
- Create multiple sessions from the same TUI run.
- Switch sessions without breaking the active render state of either session.
- Open multiple terminal instances of `crabcode` and see the same sessions and streaming statuses.
- Close a terminal while a generation is running, reopen later, and see the generation still running or completed.
- Use `/sessions` as the main session switcher, with a left-side sheet/sidebar rather than a centered modal.
- Group `/sessions` by folder/workspace, not by Today/date buckets.
- Keep workspace group ordering stable. Do not reorder groups every time a session updates; default to "whatever workspace was added first" and allow explicit reordering.
- Support an Active/All visibility toggle like Codex:
  - Active shows sessions in the current workspace plus any running/waiting sessions from any workspace.
  - All shows every unarchived workspace/session.
  - Archived sessions/workspaces are hidden unless the user explicitly opens an archive view/filter.
- Support pin/favorite. Pinned sessions are first-class navigation items, not a later nice-to-have.
- Show each session's live state: idle, loading, streaming, waiting for permission, done, failed, cancelled.
- Use the Claude-style/lazygitrs loading glyph for running sessions. The reference implementation in `/Users/carlo/Desktop/Projects/lazygitrs` uses:

```rust
const SPINNER_CHARS: &[char] = &['·', '✻', '✽', '✶', '✳', '✢'];
```

Existing `crabcode` also has `src/ui/components/wave_spinner.rs`, which is better for the chat footer. The session list probably wants the compact glyph cycle instead.

## Current State

Useful pieces already exist:

- `src/session/manager.rs` has session CRUD and an in-memory `HashMap<String, Session>`.
- `src/session/types.rs` has `Session` and `Message`.
- `src/persistence/history.rs` persists sessions/messages into SQLite.
- `src/views/sessions_dialog.rs` implements the current `/sessions` dialog.
- `src/app.rs` already wires session switching, rename/delete, chat rendering, streaming, cancellation, and completion persistence.
- `src/ui/components/wave_spinner.rs` has an existing animated loading component.

The blocking limitation is that `App` is still one-active-chat oriented:

- One `ChatState`.
- One global `is_streaming`.
- One `chunk_receiver`.
- One `streaming_cancel_token`.
- One active `base_focus`.
- Completed assistant/tool messages are persisted at stream end, while in-progress stream state mostly lives in memory.

That is fine for the current app, but it cannot support multiple concurrent sessions or cross-process streaming visibility.

## Reference Architecture

Use `/Users/carlo/Desktop/Projects/ai-studio` as the main app architecture reference for client-side session isolation.

Relevant files:

- `/Users/carlo/Desktop/Projects/ai-studio/src/contexts/active-chat.context.tsx`
- `/Users/carlo/Desktop/Projects/ai-studio/src/contexts/chat.context.tsx`
- `/Users/carlo/Desktop/Projects/ai-studio/src/pages/chat/+Layout.tsx`
- `/Users/carlo/Desktop/Projects/ai-studio/src/server/modules/chat/stream.handler.ts`
- `/Users/carlo/Desktop/Projects/ai-studio/src/server/modules/chat/chat.dao.ts`
- `/Users/carlo/Desktop/Projects/ai-studio/src/server/modules/chat/chat.controller.ts`

The key pattern to copy is the per-conversation instance registry:

- `ActiveChatContextProvider` owns the active conversation id plus a list of alive chat instances.
- Each alive `ChatInstance` has a stable key and its own mutable conversation id.
- The chat layout renders one `ChatContextProvider` per alive instance.
- Only the active provider reveals UI children; inactive providers stay mounted/headless.
- Each provider owns its own `useChat`, message state, status, error, stop handler, and stream transport.
- Switching conversations changes the active key; it does not move one stream into another conversation's UI.
- A fresh conversation can receive its server id mid-stream without remounting because the instance key stays stable.

The server side is also relevant:

- User messages are persisted immediately.
- The conversation row stores an `activeStreamId`.
- The sidebar detects streaming conversations from both local client state and server-polled `activeStreamId`.
- Finished assistant messages are saved on stream finish.
- Resumable streams let another tab/client reconnect to an active conversation stream.

For `crabcode`, this maps to `ClientSessionState` instances in the TUI plus a global runtime-backed stream owner. The TUI should preserve the ai-studio property that each session has its own state/context and cannot accidentally render another session's stream.

## Non-Goals For The First Version

- No worktree orchestration.
- No collaborative editing.
- No cloud sync.
- No remote server.
- No attempt to make filesystem mutations conflict-free.
- No full redesign of the chat renderer beyond what is needed to isolate per-session state.

Concurrent Build sessions in the same checkout are allowed. Treat them like two agents working in the same workspace; do not add complex write-guarding for this feature.

## Architecture Direction

### 1. Split Durable State From TUI State

Introduce a durable session store that can answer:

- What sessions exist for this workspace?
- Which sessions are currently running?
- What is the current transcript snapshot for a session?
- What is the current generation status?
- What live events happened since sequence N?
- Has this generation been cancelled/interrupted?

SQLite can stay the source of truth, but it needs to become streaming-aware instead of completion-only.

Suggested tables/fields:

- `workspaces`
  - `id`
  - `root_path`
  - `display_name`
  - `sort_order`
  - `archived_at`
  - `last_opened_at`
- `sessions`
  - existing fields
  - `workspace_id`
  - `status`
  - `active_generation_id`
  - `last_error`
  - `last_event_seq`
  - `pinned_at`
  - `archived_at`
- `generations`
  - `id`
  - `session_id`
  - `agent_mode`
  - `provider`
  - `model`
  - `status`
  - `started_at`
  - `ended_at`
  - `cancel_requested_at`
  - timing/token metrics
- `generation_events`
  - `id` or monotonic `seq`
  - `session_id`
  - `generation_id`
  - `kind`
  - `payload_json`
  - `created_at`
- `messages`
  - keep current transcript rows
  - allow an assistant/tool message to be incomplete
  - update or snapshot streaming content during generation

For first implementation, prefer throttled snapshots plus an event log:

- Snapshots make reload/render fast.
- Events let attached TUIs stream incrementally.
- If a client misses events, it can reload the snapshot and resume from the latest sequence.
- Do not persist every token as its own durable row unless it turns out to be necessary.
- Persist user messages immediately.
- Persist assistant/tool message snapshots during streaming on a throttle, such as every 250-500 ms, on newline boundaries, on tool state changes, and at stream end.
- Persist explicit events for status changes, permission/question waits, tool calls/results, title changes, errors, cancellation, and completion.

### 2. Add A Runtime Layer

The runtime owns active generations. The TUI should request work; it should not directly own the long-lived stream.

Possible shape:

- `crabcode` starts or connects to one global local runtime for the user.
- The runtime is app-global, not per workspace.
- Runtime uses a Unix domain socket on Unix/macOS, likely under the crabcode state dir.
- TUI clients send commands like:
  - `CreateSession`
  - `StartGeneration`
  - `SubscribeSession`
  - `CancelGeneration`
  - `ListSessions`
  - `LoadSession`
- Runtime writes all durable state to SQLite.
- Runtime broadcasts lightweight events to connected clients.
- If all clients disconnect, runtime keeps running while generations are active.
- When idle for some timeout, runtime can exit.

This can be implemented as a daemon-ish process without forcing the user to manage a service. If no runtime is found, the first `crabcode` instance starts one and attaches.

### 3. Make Session State Isolated In The TUI

The TUI needs a per-session view model instead of one global chat state.

Suggested local shape:

```rust
struct ClientSessionState {
    session_id: String,
    chat: ChatState,
    input_draft: String,
    scroll_offset: usize,
    selection: Option<...>,
    loaded_until_seq: i64,
    status: SessionStatus,
    active_generation_id: Option<String>,
    loading: bool,
}
```

Then `App` becomes closer to:

```rust
struct App {
    active_session_id: Option<String>,
    sessions: HashMap<String, ClientSessionState>,
    runtime: RuntimeClient,
    sessions_panel: SessionsPanelState,
    ...
}
```

Switching sessions should only change `active_session_id`. It should not clear/rebuild global chat state unless the session has not been loaded yet.

Each session should preserve its own input draft, scroll offset, selection state, stream status, and pending prompt state. Switching sessions should feel like switching browser tabs.

For inactive sessions, keep raw message state, stream status, pending prompt state, and transcript snapshots current, but do not keep expensive markdown/wrapping render caches hot while hidden. Rebuild visual caches when a session is focused again. This matches the `ai-studio` pattern: inactive chat providers stay alive, but inactive UI rendering does not keep paying the full render cost.

### 4. Move Streaming Flow Behind Runtime APIs

Current flow in `src/app.rs`:

- user submits message
- app appends message locally
- app creates mpsc channel
- app stores `chunk_receiver`
- app spawns `stream_llm_with_cancellation`
- app processes chunks
- app persists completed assistant/tool messages

Future flow:

- user submits message
- TUI sends `StartGeneration(session_id, user_message, agent/model/provider/cwd)`
- runtime persists the user message
- runtime starts generation worker
- runtime persists stream snapshots/events
- all attached clients receive stream events
- TUI renders active session events, and updates inactive session badges/status
- on completion, runtime marks generation/session complete

The `stream_llm_with_cancellation` function can remain useful, but it should run inside the runtime worker and publish events through a runtime sender/store instead of directly into `App`.

### 5. Sessions Panel

`/sessions` should become a left-side session switcher.

Behavior:

- Opens from any screen.
- Search/filter remains useful.
- Groups by workspace folder.
- Shows workspace groups in stable `sort_order`, with new workspaces appended by default.
- Does not reorder groups by "current" or "recent" automatically; avoid layout shifts.
- Shows pinned sessions first, then running/waiting sessions, then the rest.
- Selected row can switch active session.
- New session action creates a session in the workspace where this TUI was launched, then switches to it.
- `/new` should use the same workspace behavior.
- Delete/rename remain.
- Pin/unpin is important v1 behavior.
- Archive/unarchive should exist separately from delete.
- Loading state appears when hydrating a session snapshot.
- Visibility modes:
  - `Active`: current workspace plus sessions/workspaces that are currently running, waiting, or otherwise active.
  - `All`: every unarchived workspace/session in stable order.
  - `Archive`: archived sessions/workspaces, available through a filter/action rather than shown by default.
- Workspace group ordering can be changed explicitly:
  - Keyboard: when a workspace header is focused, `J` moves it down and `K` moves it up.
  - Mouse: drag a workspace header to reorder groups.
  - Persist the resulting `sort_order`.
- Session creation shortcut:
  - `ctrl+n` creates a new session from `/sessions`.
  - Plain `n` remains normal search input.

Row rendering should stay close to the current sessions dialog item style. Avoid right-side metrics like `23t/s`, `waiting`, `failed`, or `4 msgs` for v1. Add only one compact status marker:

- Loading glyph when the session is actively streaming/loading.
- Green circle when a completed stream has unread output that the user has not checked yet.
- No marker for ordinary idle/read sessions.

Possible row format:

```text
~/Projects/crabcode
  ✻ Fix model picker persistence
  ● Review config docs
    Update model picker

~/Projects/lazygitrs
  ✽ Generate branch UI patch
```

This should avoid date grouping. Recency can still decide sort order within a folder.

### 6. Interruption Model

Interruption should stay focused on the active chat for v1:

- Active session interruption: `Esc` cancels the active generation.
- `/sessions` interruption: none for v1. The sessions panel is navigation, not a stop UI.

Possible command/shortcut names:

- `Esc` in chat: cancel active generation.
- `Esc` in `/sessions`: close the panel only; do not stop a running session.
- `/stop` can remain as a command alias later if useful, but the primary UX is `Esc`.

Runtime cancellation should be durable:

- mark `generations.cancel_requested_at`
- signal the worker cancellation token if the worker is local/alive
- let other clients immediately show `cancelling`
- finalize as `cancelled` when the worker confirms
- recover stale `cancelling/running` rows on runtime restart by marking them `failed` or `interrupted`, depending on policy

Permission and question waits are not cancellation:

- If a tool asks for permission and no TUI is attached, pause indefinitely.
- Persist the pending request id, prompt, options, generation id, and session id.
- Mark the session/generation as `waiting_permission` or `waiting_question`.
- When any TUI reconnects, `/sessions` should show the waiting state and entering the session should surface the prompt.
- While paused, the model is not consuming tokens.
- If the runtime itself exits while waiting, recovery may need to mark the generation as interrupted unless the provider/runtime stream can be resumed safely. The target behavior is still indefinite pause while the runtime is alive.
- If the global runtime exits, treat it like every active generation received `Esc`: mark running/waiting/cancelling generations as `interrupted`.

### 7. Cross-Process Consistency

Multiple TUI clients should be able to attach without corrupting state.

Rules:

- SQLite should use WAL mode.
- A generation has exactly one owner worker.
- Runtime should acquire a lease/lock before starting a generation.
- Session/message writes go through the runtime where possible.
- Direct SQLite reads are fine for snapshots, but commands that mutate running state should go through IPC.
- Runtime heartbeats allow stale worker detection.

Important case:

- Terminal A starts a generation.
- Terminal B opens `/sessions`.
- Terminal B sees the same session as streaming.
- Terminal B switches to it and receives snapshot plus live events.
- Terminal A exits.
- Runtime keeps the generation going.
- Terminal B can switch into the session and press `Esc` from the chat if it needs to interrupt it.

Because the runtime is global, the same process owns running generations across all workspace folders. Workspaces are only grouping and context boundaries, not runtime boundaries.

If the global runtime crashes or is intentionally stopped, all active generations should recover as interrupted. This is equivalent to every client pressing `Esc` at the same time.

## Migration Path

### Phase 0: References And Terms

- Use `/Users/carlo/Desktop/Projects/ai-studio` as the client-state reference.
- Treat "workspace" as folder/project root for this version.
- Use one global runtime for all workspaces.

### Phase 1: Streaming-Aware Persistence

- Add session/generation status fields.
- Persist incomplete assistant/tool messages.
- Add event sequence or generation event rows.
- Keep current single active TUI behavior.
- Acceptance: killing/restarting the TUI can show an incomplete or failed generation cleanly.

### Phase 2: Per-Session View State In One Process

- Replace one global `ChatState` with `HashMap<SessionId, ClientSessionState>`.
- Move global `is_streaming` into per-session status.
- Allow switching sessions while one is running.
- Keep runtime in-process for this phase.
- Acceptance: two sessions can exist in one TUI and switching does not clear scroll/render/input state.

### Phase 3: Sessions Panel Redesign

- Move `/sessions` to a left-side panel.
- Group by workspace/folder.
- Add Active/All/Archive visibility modes.
- Keep workspace groups in stable insertion/sort order.
- Add `J`/`K` and mouse-drag reordering for workspace groups.
- Add loading/running/waiting/failed/done indicators.
- Add `ctrl+n` new session, pin/unpin, archive/unarchive actions.
- Use compact loading glyphs for running rows.
- Use a green unread-complete marker for sessions that finished streaming while not focused.
- Acceptance: the panel is the primary navigation for sessions.

### Phase 4: Runtime Client Boundary

- Introduce `RuntimeClient` and `RuntimeEvent`.
- Move stream start/cancel/list/load behind the boundary.
- Keep an in-process runtime implementation first so the app compiles through the refactor.
- Acceptance: `App` no longer directly owns generation workers.

### Phase 5: Local Background Runtime

- Add socket IPC.
- Auto-start the global runtime if missing.
- Let runtime keep active generations alive after the TUI exits.
- Let multiple TUI clients subscribe to the same sessions.
- Acceptance: Terminal B can watch a generation started in Terminal A, switch into that chat, and interrupt it with `Esc`.

### Phase 6: Recovery And Polish

- Add stale runtime/generation recovery.
- Runtime-exit recovery marks active generations as interrupted.
- Verify concurrent Build sessions behave understandably in one checkout.
- Add better session loading states.
- Add tests around event replay, cancellation, duplicate worker prevention, and session switching.
- Add tests for waiting permission/question recovery while runtime stays alive.
- Acceptance: closed terminals, crashed clients, and restarted runtimes leave understandable session state.

## Main Risks

- Shared checkout conflicts if multiple Build sessions edit files at the same time.
- Persisting every stream chunk may be too chatty; throttled snapshots plus events may be better.
- Tool permission dialogs become harder when the generating session is not active or no TUI is attached.
- Cross-process runtime bugs are more expensive than local UI bugs.
- Session title/metadata updates can race unless runtime owns writes.

## Acceptance Criteria

- `/sessions` shows sessions grouped by folder/workspace.
- Running sessions have an animated status indicator.
- Sessions that finished streaming in the background show a green unread-complete marker until checked.
- A user can start session A, switch to session B, and session A keeps streaming.
- Returning to session A shows the stream where it is, not a reset or stale copy.
- Another `crabcode` terminal can see the same running session.
- Closing the original terminal does not stop an active generation.
- A running session can be interrupted from the active chat with `Esc`.
- `/sessions` `Esc` closes only the panel and never stops a running session.
- Completed, failed, and cancelled generations survive restart with clear status.
- Permission/question waits can pause without a TUI attached and resume when a TUI opens the session.
- Workspace group order stays stable until the user reorders it.
- The sessions panel can switch between Active, All, and Archive views.
- Sessions can be pinned/favorited.
- Session and workspace archive are both supported.
- Each session preserves its own input draft, scroll, selection, and pending UI state.
- Inactive streaming sessions keep raw state live and rebuild visual render caches when focused.
