# Remote access planning notes

Internal planning note. This is not public user guidance yet, and it should stay out of `gittydocs.jsonc` navigation until we decide what is safe and stable enough to document.

Last updated: 2026-05-21.

## Product Goal

Make crabcode usable when the machine that owns the workspace is not the machine in the user's hands.

The important cases are:

- A developer uses crabcode installed on a VPS, desktop, homelab box, or laptop from another computer.
- A developer controls crabcode from a phone while away from the keyboard.
- A developer uses an iPad or other tablet to SSH into a VPS/Mac mini, starts crabcode remote access, then uses the tablet browser to control crabcode and view the app being developed on the remote machine.
- A developer starts a long agent run, disconnects, and later resumes from another device without losing stream state.
- A developer can use this safely without exposing a write-capable coding agent directly to the public internet.

This should stay terminal-first. Remote access is about reaching the same terminal-native agent from more places, not turning crabcode into a hosted cloud product.

Scope decision: this is personal-device and personal-VPS access for now. Team/shared access, workspace collaboration, and multi-user permission models are later problems.

## Recommendation

Use three lanes:

1. Support what already works: SSH into the remote machine, run `crabcode` inside `tmux` or `zellij`, and document Tailscale as the preferred private network path.
2. Turn crabcode into a backend service the first time `crabcode` is run. The TUI becomes one client of that backend, and active generations can outlive any one TUI client.
3. After the backend exists, add a visible `crabcode serve` command for phone and browser access, bound privately by default. The browser surface should be a minimal touch-native frontend backed by crabcode's service API.

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
- There is no HTTP server, websocket server, or remote client protocol.
- There is no durable active-generation owner. If the TUI process dies, the active stream dies with it.

The existing multiworkspace plan already points at the architectural prerequisite: split durable session state from TUI state and add a runtime that owns active generations.

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

### Mode B: Local backend service

This is the real product foundation.

The first time `crabcode` runs, it should ensure a local crabcode backend service exists, then attach a TUI client to it. The backend owns active generations, persists events, and lets clients attach and detach. It does not have to be a user-managed service.

Expected shape:

- Runtime socket under the crabcode state dir, such as `~/.local/state/crabcode/runtime.sock`.
- Runtime starts on demand and exits after an idle timeout once there are no connected clients and no active generations.
- TUI, browser, and future clients send commands:
  - `ListWorkspaces`
  - `ListSessions`
  - `CreateSession`
  - `LoadSession`
  - `StartGeneration`
  - `CancelGeneration`
  - `ApprovePermission`
  - `AnswerQuestion`
  - `SubscribeSession`
- Runtime writes durable state:
  - user messages immediately
  - generation rows for active turns
  - throttled assistant/tool snapshots during streaming
  - explicit events for status changes, permission waits, question waits, tool calls, tool results, errors, cancellation, and completion
- Clients can disconnect without killing active generations.
- Multiple devices can control the same session.
- Permission/question prompts can be answered from any connected controlling device. The backend must make approval state idempotent so the first answer wins and later duplicate answers become no-ops.
- Connected controlling clients should show presence/activity for the same session, such as "phone attached", "desktop attached", "Carlo approved bash", or "desktop is typing".

This also fixes local multi-terminal usage, not just remote usage.

### Mode C: Minimal Browser Frontend

Only build this after Mode B exists.

The first real phone surface should be a small web frontend that talks to the crabcode backend API. Do not adapt the current TUI directly for the browser: crabcode's current TUI is keyboard-driven, while the phone use case is touch, mobile text entry, and quick glance/control while away from the keyboard.

The server can be in the same `crabcode` binary:

```bash
crabcode serve --bind 127.0.0.1:8421
```

Potential Tailscale workflow:

```bash
crabcode serve --bind 127.0.0.1:8421
tailscale serve --bg http://127.0.0.1:8421
```

Minimal useful slice:

- Authenticate/pair the browser.
- Show workspace/session list.
- Create a new session/conversation.
- Load a session transcript with live streaming updates.
- Type and send a new prompt from the phone.
- Stop/cancel an active generation.
- Approve/deny permission prompts.
- Answer model questions.
- Show or remember externally reachable dev preview URLs, such as a Tailscale host URL, SSH-forwarded URL, or another tunnel URL.
- Show current workspace, model, agent mode, remote host, and connected devices.
- Show simple presence/activity for other controlling clients.

Why this is a good first slice:

- It gives touch-native controls for the actions a phone user actually needs.
- It uses the same durable backend protocol the TUI needs anyway.
- It avoids making users learn terminal gestures or keyboard shortcuts on glass.
- It can grow toward CLI-equivalent control without pretending the phone is a terminal.

Risks:

- It requires designing and maintaining a small frontend.
- Every CLI feature we expose needs a backend API shape.
- We need to be careful not to accidentally build a full web IDE.

This frontend should be intentionally narrow: it is a remote controller for crabcode sessions, not a replacement IDE.

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

This keeps the security boundary clearer. crabcode controls crabcode sessions; network/tunnel tools expose dev servers.

## Security Defaults

Remote crabcode is write-capable by design, so defaults must be conservative.

- Bind localhost only unless the user explicitly passes a non-local bind address.
- Never recommend opening a crabcode HTTP port directly to the public internet.
- Require authentication for any HTTP/browser access, even on a tailnet.
- Use a short-lived pairing code for new browser clients.
- Store trusted browser clients separately from provider credentials.
- Include CSRF and Origin checks for browser routes.
- Use backend API routes for the mobile frontend. Do not expose a browser terminal.
- Do not expose arbitrary remote ports. crabcode should not become a general-purpose open proxy.
- Show remote host, cwd, model, agent, and pending command/file changes in permission prompts.
- Keep provider credentials on the machine running crabcode. Do not sync `auth.json` between devices in the first design.
- Log remote approvals and denials into the session event stream.
- Allow multiple controlling devices for the same session, but make state-changing operations idempotent and auditable.
- Show presence/activity for connected controlling devices.
- Shut the backend down after an idle timeout when there are no clients and no active generations.
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

## Implementation Plan

### Phase 0: Internal planning

- Keep this document internal.
- Target both remote-computer access and phone control.
- Treat phone access as a full remote control surface, not read-only monitoring.
- Keep the scope personal-device/personal-VPS for now.
- Make `crabcode serve` visible in help.
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

### Phase 2: Runtime architecture

- Introduce a local backend process and IPC protocol.
- Start or connect to the backend on the first normal `crabcode` run.
- Persist generation status and event stream in SQLite.
- Move active stream ownership out of `App`.
- Make the TUI subscribe to session events.
- Preserve per-session client view state in the TUI.
- Support detach/reconnect for active generations.
- Support multiple controlling clients on the same session.
- Emit presence/activity events for attached clients.
- Exit the backend after an idle timeout.

This phase should reuse the multiworkspace plan rather than becoming a parallel architecture.

### Phase 3: Remote API

- Add visible `crabcode serve` CLI help.
- Start with localhost binding only.
- Add token/pairing auth.
- Add WebSocket or SSE event streaming for sessions/generations.
- Add backend API routes for session list/create/load, prompt submit, cancel, approve/deny, answer question, model/agent metadata, and presence.
- Add tests for auth, disconnect/reconnect, idle shutdown, multiple controlling clients, permission idempotency, and event replay.
- Add explicit command-line warnings when binding to non-local addresses.

### Phase 4: Minimal Browser/PWA client

- Serve a small static web client from the binary or bundled assets.
- Optimize for phone/tablet first: session list, new session, transcript, input, approvals, questions, stop, and saved external preview links.
- Grow toward CLI-equivalent control from the browser.
- Keep advanced TUI-only workflows in SSH only as temporary gaps.
- Add installable PWA metadata only if the mobile browser experience is good.

### Phase 5: Revisit native app

Only consider a separate app if:

- Mobile browser input is not good enough.
- Notifications/background behavior matters enough to justify native code.
- Pairing and secure storage are stable.
- The runtime protocol has stopped changing quickly.

## Open Questions

- Do remote approvals need a stricter permission mode than local approvals?
- Should remote browser clients be allowed to run every slash command, or should some commands stay TUI-only at first?
- What idle timeout should the backend use?
- What is the smallest complete mobile command surface after sessions, prompt input, cancel, approvals, questions, and transcript?
- Should crabcode auto-detect dev-server URLs from terminal output, or only let users manually add/pin them?

## Non-Goals For Now

- Hosted crabcode cloud.
- Public internet sharing.
- Collaborative editing.
- Syncing provider credentials across devices.
- A separate native mobile app.
- A desktop app.
- Multi-user team permissions.
