# Remote access planning notes

Internal planning note. This is not public user guidance yet, and it should stay out of `gittydocs.jsonc` navigation until we decide what is safe and stable enough to document.

Last updated: 2026-05-31.

## Product Goal

Make crabcode usable when the machine that owns the workspace is not the machine in the user's hands.

The product model should be simple enough to remember:

> One machine hosts the workspace. Every other device attaches.

The important cases are:

- A developer uses crabcode installed on a VPS, desktop, homelab box, or laptop from another computer.
- A developer controls crabcode from a phone while away from the keyboard.
- A developer uses an iPad or other tablet to SSH into a VPS/Mac mini, starts crabcode remote access, then uses the tablet browser to control crabcode and view the app being developed on the remote machine.
- A developer starts a long agent run, disconnects, and later resumes from another device without losing stream state.
- A developer starts crabcode on a host, then can use either a phone browser, another laptop TUI, or a quick non-interactive prompt against the same host URL.
- A developer can use this safely without exposing a write-capable coding agent directly to the public internet.

This should stay terminal-first. Remote access is about reaching the same terminal-native agent from more places, not turning crabcode into a hosted cloud product.

Scope decision: this is personal-device and personal-VPS access for now. Team/shared access, workspace collaboration, and multi-user permission models are later problems.

## Recommendation

Make remote access a first-class host/client shape:

```bash
# Host machine / VPS
crabcode serve

# Phone / tablet
# Open the printed browser URL.

# Another laptop
crabcode attach <url>

# Script, launcher, or quick remote prompt
crabcode -p --attach <url> "continue the refactor"
```

`crabcode serve` is the primitive. It owns the workspace, credentials, session history, active generations, permission prompts, and pairing. Browser access, `crabcode attach`, and `crabcode -p --attach` are clients of the same service protocol.

Support what already works first: SSH into the remote machine, run `crabcode` inside `tmux` or `zellij`, and document Tailscale as the preferred private network path. But the target product should not stop at SSH. The ten-out-of-ten version is a host URL that can be used from a phone browser, a remote terminal client, or a non-interactive CLI prompt.

The first polished host output should look like:

```text
crabcode host ready

Workspace: /home/carlo/project
Browser:   http://devbox:8421
Attach:    crabcode attach http://devbox:8421
Prompt:    crabcode -p --attach http://devbox:8421 "..."
Pair:      482-119  expires in 10 minutes
```

The browser UI and `attach` TUI must not become separate implementations. Build one authenticated host API and event stream, then put thin clients on top:

- Phone browser: touch-first prompt, approve, cancel, and preview-link control.
- `crabcode attach <url>`: terminal-native remote TUI that feels like local crabcode, with clear remote host/cwd/model status.
- `crabcode -p --attach <url>`: non-interactive remote prompt for scripts, aliases, launchers, and quick follow-ups.
- Future clients: native app, desktop app, shortcuts, or automation can reuse the same protocol if they become worth building.

Do not build a separate native app yet. A separate app adds release, auth, pairing, mobile UX, and protocol maintenance before we even know if the runtime protocol is correct. If mobile browser limitations become the real blocker, revisit a native app after the web companion exists.

Recommend Tailscale, but do not require it. The default docs should say "use SSH over Tailscale if you can; plain SSH with key auth is also fine on a hardened VPS." Tailscale is one example of a private overlay network: a way to make selected personal devices able to reach each other without exposing services to the whole internet. Similar options include WireGuard-based setups, ZeroTier, NetBird, and Cloudflare Access/Tunnel-style SSH access. Tailscale is the easiest default to explain, but crabcode should only require ordinary network reachability plus its own auth.

For browser access, prefer a private overlay network or localhost tunnel. Treat public exposure as out of scope for write-capable crabcode access unless we later add strong auth, clear warnings, and a narrow sharing mode.

## Current State

crabcode is currently a local TUI process:

- `src/main.rs` owns raw terminal setup, alternate screen, crossterm events, and the main event loop.
- `src/app.rs` owns the active TUI state, session state, streaming state, dialogs, permissions, and model selection.
- `src/persistence/history.rs` persists workspaces, sessions, and messages to SQLite.
- `crabcode -s <session_id>` resumes an existing session after process restart.
- `crabcode -p "<prompt>"` supports non-interactive print mode.
- There is no `crabcode serve`, `crabcode attach <url>`, or `crabcode -p --attach <url>`.
- There is no HTTP server, websocket server, or remote client protocol.
- There is no durable active-generation owner. If the TUI process dies, the active stream dies with it.

The existing multiworkspace plan already points at the architectural prerequisite: split durable session state from TUI state and add a runtime that owns active generations. The remote plan should reuse that split instead of building a parallel web-only architecture.

## Usage Modes

### Mode A: SSH + terminal multiplexer

This should be the first documented remote usage because it requires almost no crabcode changes.

Expected workflow:

```bash
tailscale ssh devbox
cd ~/code/project
tmux new -A -s crabcode
crabcode
```

Or with normal SSH:

```bash
ssh devbox
cd ~/code/project
tmux new -A -s crabcode
crabcode
```

This works for another PC and can work from a phone using a mobile SSH client. The phone experience will be constrained by terminal input, keyboard shortcuts, screen size, and copy/paste, but it is the safest first answer.

Immediate polish work for this mode:

- Make `/connect` fully usable on headless machines. Browser OAuth needs a copyable URL and code path, and API-key auth needs to be comfortable over SSH.
- Make terminal resize behavior reliable on small screens.
- Keep `crabcode -s <session_id>` prominent after exit.
- Consider a short "remote terminal checklist" in public docs later: install, authenticate, use `tmux`, use Tailscale or hardened SSH, avoid running as root.
- Decide whether sounds and desktop notifications should auto-disable or degrade cleanly when running over SSH.

### Mode B: Host service

This is the real product foundation.

`crabcode serve` starts the host runtime for the current workspace. The host owns active generations, persists events, serves the phone UI, accepts terminal attaches, and lets clients disconnect/reconnect without killing work.

Expected workflow:

```bash
cd ~/code/project
crabcode serve
```

Expected output:

```text
crabcode host ready

Workspace: /home/carlo/project
Browser:   http://devbox:8421
Attach:    crabcode attach http://devbox:8421
Prompt:    crabcode -p --attach http://devbox:8421 "..."
Pair:      482-119  expires in 10 minutes
```

Expected shape:

- Runtime socket under the crabcode state dir for local clients, such as `~/.local/state/crabcode/runtime.sock`.
- HTTP API and event stream for browser/remote clients.
- Host starts explicitly with `crabcode serve`; a later local daemon can also start on demand when normal `crabcode` runs.
- Host exits only when explicitly stopped, or after a configured idle timeout if daemon mode is added later.
- Host keeps provider credentials on the machine running crabcode.
- Host exposes no general-purpose terminal and no arbitrary port proxy.
- Host emits durable session events and status snapshots so clients can replay state after reconnecting.
- Host prints a browser URL, attach command, print-mode attach command, short-lived pairing code, and ideally a terminal QR code.

Host commands accepted from clients:

- `ListWorkspaces`
- `ListSessions`
- `CreateSession`
- `LoadSession`
- `StartGeneration`
- `CancelGeneration`
- `ApprovePermission`
- `DenyPermission`
- `AnswerQuestion`
- `SubscribeSession`
- `ListPreviewLinks`
- `SavePreviewLink`
- `ListClients`

Host-written durable state:

- user messages immediately
- generation rows for active turns
- throttled assistant/tool snapshots during streaming
- explicit events for status changes, permission waits, question waits, tool calls, tool results, errors, cancellation, completion, preview-link changes, and client attach/detach

This also fixes local multi-terminal usage, not just remote usage.

### Mode C: Remote clients

Build browser, terminal attach, and print-mode attach against the same host protocol.

#### Phone browser

The phone surface should be a small touch-native frontend. Do not adapt the current TUI directly for the browser: crabcode's current TUI is keyboard-driven, while the phone use case is mobile text entry, quick glance/control, and approvals while away from the keyboard.

Minimal useful slice:

- Authenticate/pair the browser.
- Show workspace/session list.
- Create a new session/conversation.
- Load a session transcript with live streaming updates.
- Type and send a new prompt from the phone.
- Stop/cancel an active generation.
- Approve/deny permission prompts.
- Answer model questions.
- Show current workspace, model, agent mode, remote host, and connected devices.
- Show simple presence/activity for other controlling clients.
- Show and pin external dev preview URLs.

The phone client should default toward a prompt/approve role, not a full IDE role. It can grow later, but v1 should make the common away-from-keyboard path excellent.

#### Terminal attach

`crabcode attach <url>` should launch a remote TUI client for another laptop or terminal-capable tablet.

Expected workflow:

```bash
crabcode attach http://devbox:8421
```

It should feel like local crabcode, with persistent remote context in the UI:

```text
remote: devbox  cwd: /home/carlo/project  model: opencode/big-pickle
```

Attach should support the same core actions as the local TUI: session list, transcript, prompt input, cancel, approvals, questions, model/agent metadata, and preview links. Advanced local-only workflows can remain SSH-only temporarily, but the attach path should be treated as a first-class client rather than a debugging convenience.

#### Print-mode attach

`crabcode -p --attach <url> "..."` should submit a prompt to the host and stream the result to stdout.

Expected workflow:

```bash
crabcode -p --attach http://devbox:8421 "continue the next step"
```

This is useful for scripts, aliases, launchers, phone shortcuts, and quick prompts from a second laptop. It is also the simplest client and should be implemented early as a protocol proving ground before the full remote TUI.

#### Remembered hosts

After a successful pairing, users should be able to name and reuse hosts:

```bash
crabcode hosts
crabcode attach devbox
crabcode -p --attach devbox "summarize the current state"
```

Remembered hosts should store host URL, display name, trust token, last-used time, and possibly a friendly workspace label. They must not store provider credentials.

### Mode D: Remote dev previews

The iPad/VPS workflow should be possible:

1. SSH into the VPS or Mac mini from an iPad terminal app.
2. Start the project and crabcode remote access from the remote shell.
3. Open the paired crabcode browser UI on the iPad.
4. Use crabcode from the browser and preview the app running on the remote machine.

Important network detail: `localhost:3000` in the iPad browser means the iPad, not the VPS/Mac mini. To view a dev server running on the remote host, the browser needs one of these:

- Private-network direct access to the remote host and port, such as `http://devbox:3000`, with the dev server bound to a reachable interface.
- SSH local port forwarding from the iPad SSH client, if that client supports it reliably.
- A separate tunnel or proxy tool intended for app previews.

Product boundary: crabcode should not own serving arbitrary dev-server ports.

crabcode can help by:

- Documenting the common options: Tailscale/private-network direct access, SSH local forwarding, or external tunnel tools.
- Letting the user save or pin preview URLs in the remote UI.
- Detecting likely dev-server URLs from command output when practical and presenting them as links.
- Warning when a preview URL points at the local browser's `localhost`, because that usually is not the remote devbox.
- Offering a preview-link panel in the browser and attach TUI, scoped to the active workspace/session.
- Supporting simple localhost-to-remote hints, such as "the host printed `localhost:3000`; try `http://devbox:3000` or your Tailscale host URL."

This keeps the security boundary clearer. crabcode controls crabcode sessions; network/tunnel tools expose dev servers.

## Product Details That Make This Feel Native

### Pairing

Pairing should be fast and obvious:

- Print the browser URL.
- Print the attach command.
- Print the `-p --attach` command shape.
- Print a short pairing code with an expiry timer.
- Print a terminal QR code when the terminal can reasonably display it.
- Remember trusted devices after pairing.
- Let the user revoke trusted devices from the host.

### Device roles

Clients should have explicit roles:

- `phone`: prompt, cancel, approve/deny, answer questions, and manage preview links.
- `attach-tui`: full terminal control where implemented.
- `print`: submit one prompt and stream one answer.
- `monitor`: read-only transcript/event stream, later if needed.

The role should be shown in presence/activity and approval audit logs. The phone role should be the safe default for browser clients because it matches the "continue prompting from my phone" use case without implying a full browser IDE.

### Presence and audit trail

Connected clients should be visible in the session:

- "phone attached"
- "desktop attached"
- "Carlo approved bash from phone"
- "desktop submitted prompt"
- "phone disconnected"

Presence is not collaboration yet. It is there to reduce ambiguity when multiple personal devices are controlling one write-capable agent.

### Host aliases

The first pairing can optionally save a friendly host alias:

```bash
crabcode attach devbox
crabcode -p --attach devbox "what is currently running?"
```

Aliases should be local to the attaching device and map to a URL plus trust token. They should be easy to list, rename, and forget.

## Protocol Shape

Use one event/API contract for the browser UI, `crabcode attach`, and `crabcode -p --attach`.

Session event envelope:

```text
SessionEvent {
  seq
  session_id
  generation_id?
  actor_client_id?
  kind
  payload
  created_at
}
```

Important event kinds:

- `client_attached`
- `client_detached`
- `user_message_added`
- `generation_started`
- `assistant_delta`
- `assistant_snapshot`
- `assistant_completed`
- `tool_started`
- `tool_updated`
- `tool_completed`
- `permission_requested`
- `permission_answered`
- `question_requested`
- `question_answered`
- `generation_cancelled`
- `generation_failed`
- `preview_link_saved`
- `preview_link_removed`

Replay rule: a client should be able to load the latest session snapshot, subscribe from a known `seq`, and recover cleanly after browser sleep, SSH drop, laptop close, or mobile network changes.

## Security Defaults

Remote crabcode is write-capable by design, so defaults must be conservative.

- Bind localhost only unless the user explicitly passes a non-local bind address.
- Never recommend opening a crabcode HTTP port directly to the public internet.
- Require authentication for browser, attach, and print-mode attach clients, even on a tailnet.
- Use a short-lived pairing code for new clients.
- Store trusted client tokens separately from provider credentials.
- Include CSRF and Origin checks for browser routes.
- Use backend API routes for the mobile frontend. Do not expose a browser terminal.
- Do not expose arbitrary remote ports. crabcode should not become a general-purpose open proxy.
- Show remote host, cwd, model, agent, and pending command/file changes in permission prompts.
- Show the requesting/approving device role in permission prompts and audit events.
- Let the host restrict client roles, such as phone prompt/approve only, attach TUI full control, or monitor read-only.
- Keep provider credentials on the machine running crabcode. Do not sync `auth.json` between devices in the first design.
- Log remote approvals and denials into the session event stream.
- Allow multiple controlling devices for the same session, but make state-changing operations idempotent and auditable.
- Show presence/activity for connected controlling devices.
- If daemon mode is enabled later, shut the backend down after an idle timeout when there are no clients and no active generations.
- Treat public internet sharing as a non-goal for now. A private overlay network is acceptable; a public URL to a write-capable coding agent is not the default shape.

## Private Network Position

Recommend Tailscale as the easiest private-network path, especially for phone and homelab/VPS use. It should be framed as one recommended option, not a hard dependency.

Good default wording for public docs later:

> For personal remote access, use SSH over Tailscale or another private network when possible. It keeps crabcode reachable only from your selected devices. A normal SSH setup with key auth is also fine on a hardened VPS.

Docs we should reference later:

- Tailscale SSH: https://tailscale.com/docs/features/tailscale-ssh
- Tailscale Serve and Funnel CLI: https://tailscale.com/docs/reference/tailscale-cli/funnel
- Tailscale access controls/grants: https://tailscale.com/kb/1018/acls
- WireGuard quick start: https://www.wireguard.com/quickstart/
- ZeroTier remote access docs: https://docs.zerotier.com/remotedesktop/
- NetBird SSH docs: https://docs.netbird.io/how-to/ssh
- Cloudflare browser SSH docs: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/use-cases/ssh/ssh-browser-rendering/

Implementation implication: crabcode does not need to integrate with Tailscale or any private-network provider APIs for v1. We only need to avoid fighting them by binding cleanly to localhost or a chosen address and by documenting the safe path.

Possible convenience later:

```bash
crabcode serve --bind 127.0.0.1:8421
crabcode serve --bind 100.x.y.z:8421
crabcode serve --bind tailnet
```

`--bind tailnet` should only exist if we can make the behavior predictable. Otherwise, keep the primitive explicit and let users pass the address they want.

## Implementation Plan

### Phase 0: Internal planning

- Keep this document internal.
- Target the host/client model: one machine hosts, every other device attaches.
- Remote v1 should include `crabcode serve`, phone browser access, `crabcode -p --attach`, and `crabcode attach`.
- Treat phone access as a full remote control surface, not read-only monitoring.
- Keep the scope personal-device/personal-VPS for now.
- Make `crabcode serve`, `crabcode attach`, and `crabcode -p --attach` visible in help once implemented.
- Use a minimal touch-native frontend as the first browser slice.
- Do not pursue a browser terminal path for this plan.

### Phase 1: Document and polish SSH usage

- Public docs page later: "Remote usage".
- Recommended path: Tailscale plus SSH plus `tmux`.
- Plain SSH path for VPS users.
- Phone path using mobile Tailscale plus a mobile SSH client.
- Add headless auth notes.
- Add a warning that credentials and filesystem access live on the remote host.

Code polish candidates:

- Better remote/headless `/connect`.
- Better small-screen layout behavior.
- Better post-exit resume instructions.
- Clearer behavior for sounds, notifications, and clipboard over SSH.

### Phase 2: Shared protocol boundary

Define the host/client contract before building individual clients.

- Add typed command and event enums for the host API.
- Add a session event envelope with monotonic `seq`.
- Add session snapshot/replay semantics.
- Add client identity, client role, and trusted-client token types.
- Add preview-link data types.
- Add idempotency keys for approvals, questions, prompt submits, and cancellation.
- Keep the protocol transport-agnostic enough that local socket, HTTP/SSE, WebSocket, and future native clients can share the same command/event model.

This phase should be mostly internal Rust types and adapters. It is the guardrail that keeps browser, attach TUI, and print attach from becoming three separate products.

### Phase 3: Host runtime and `crabcode serve`

- Add visible `crabcode serve`.
- Start with localhost binding by default.
- Add explicit command-line warnings when binding to non-local addresses.
- Print browser URL, attach command, print-mode attach command, pairing code, and QR code if feasible.
- Add token/pairing auth.
- Add HTTP routes for session list/create/load, prompt submit, cancel, approve/deny, answer question, model/agent metadata, client presence, and preview links.
- Add SSE or WebSocket event streaming for sessions/generations.
- Move active stream ownership out of `App` and into the host runtime enough for remote clients to control it.
- Persist generation status and event stream in SQLite.
- Make permission/question prompts answerable from any connected controlling device.
- Make approvals/questions idempotent so the first answer wins and duplicate answers become no-ops.
- Emit presence/activity events for attached clients.

This phase should reuse the multiworkspace/session persistence work rather than becoming a parallel remote-only runtime.

### Phase 4: Print-mode attach

Implement the simplest non-browser client first:

```bash
crabcode -p --attach http://devbox:8421 "continue the next step"
```

- Resolve host aliases as well as raw URLs.
- Pair if the host does not already trust this client.
- Submit one prompt to the selected/default session.
- Stream assistant output to stdout.
- Surface permission/question waits clearly, with a compact approval path if interactive stdin is available.
- Exit with useful status codes for cancelled, failed, denied, and completed turns.

This is the fastest way to prove the host protocol from outside the original TUI.

### Phase 5: Minimal Browser/PWA client

- Serve a small static web client from the binary or bundled assets.
- Optimize for phone/tablet first: session list, new session, transcript, input, approvals, questions, stop, and saved external preview links.
- Default browser clients to a phone-style role: prompt, cancel, approve/deny, answer questions, and preview links.
- Show connected devices and recent remote activity.
- Show host, cwd, model, agent, and pending command/file details prominently around approval prompts.
- Grow toward CLI-equivalent control from the browser only where it helps the phone/tablet workflow.
- Keep advanced TUI-only workflows in SSH only as temporary gaps.
- Add installable PWA metadata only if the mobile browser experience is good.

### Phase 6: Terminal attach

Implement the richer terminal client:

```bash
crabcode attach http://devbox:8421
crabcode attach devbox
```

- Render a TUI client backed by host snapshots and session events.
- Keep remote context visible in the status line: host, cwd, model, agent, and attached client role.
- Support session list, transcript, input, streaming, cancel, approvals, questions, model/agent metadata, and preview links.
- Preserve local UI state on the attaching machine where appropriate, but keep canonical session/generation state on the host.
- Handle reconnects after SSH drops, laptop sleep, or network changes by replaying from the last seen event `seq`.

### Phase 7: Local daemon mode

Once the explicit host/client shape works, consider making normal local `crabcode` attach to an on-demand local backend too.

- Start or connect to a local backend on normal `crabcode` runs.
- Use a runtime socket under the crabcode state dir, such as `~/.local/state/crabcode/runtime.sock`.
- Let local TUI clients detach/reconnect without killing active generations.
- Exit the daemon after an idle timeout when there are no clients and no active generations.
- Keep this as an evolution of `crabcode serve`, not a separate architecture.

### Phase 8: Revisit native app

Only consider a separate app if:

- Mobile browser input is not good enough.
- Notifications/background behavior matters enough to justify native code.
- Pairing and secure storage are stable.
- The runtime protocol has stopped changing quickly.

## Product Completeness Criteria

The plan is only a 10/10 if the first complete remote story feels like this:

- A host can run `crabcode serve` and immediately see the browser URL, attach command, print-mode attach command, and pairing code.
- A phone can pair from the printed URL or QR code and submit a prompt without touching SSH.
- A phone can approve/deny a permission request with enough context to understand the host, cwd, command/file target, model, and requesting device.
- Another laptop can run `crabcode -p --attach <url> "..."` and stream a remote answer.
- Another laptop can run `crabcode attach <url>` and get a real remote TUI client.
- Closing the phone browser, losing SSH, or sleeping the attaching laptop does not kill the active generation.
- Reattaching shows the current transcript and generation state without duplicated approvals or corrupted history.
- Preview URLs are visible and pinnable, but crabcode does not proxy arbitrary dev-server ports.
- Remembered hosts make repeat use short: `crabcode attach devbox` and `crabcode -p --attach devbox "..."`.

## Open Questions

- Do remote approvals need a stricter permission mode than local approvals?
- Which client roles should be available in v1, and should browser clients default to `phone` rather than full control?
- Should remote browser clients be allowed to run every slash command, or should some commands stay attach/SSH-only at first?
- What idle timeout should daemon mode use, if explicit `crabcode serve` does not auto-exit?
- What is the smallest complete mobile command surface after sessions, prompt input, cancel, approvals, questions, and transcript?
- Should crabcode auto-detect dev-server URLs from terminal output, or only let users manually add/pin them?
- Should event streaming use SSE first, WebSocket first, or both behind the same internal event API?
- How should host aliases be named when one host serves multiple workspaces over time?

## Non-Goals For Now

- Hosted crabcode cloud.
- Public internet sharing.
- Collaborative editing.
- Syncing provider credentials across devices.
- A separate native mobile app.
- A desktop app.
- Multi-user team permissions.
