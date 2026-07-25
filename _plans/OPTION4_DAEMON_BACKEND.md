# Option 4 — Daemon backend (OpenCode-shaped crabcode)

Internal plan. Complements `__remote-usage-plan.md` (product/remote usage) and supersedes **shipping** path in `INPROCESS_REMOTE_CAFE.md` (cafe UX decisions still apply).

Last updated: 2026-07-25.  
Status: **chosen path** — implement this; do not ship option 2 as an intermediate product.

---

## 1. Why option 4 now

We already locked the hard product decisions for the cafe story (shared control, mid-stream join, multi-session, fail-closed, status dialog). The remaining choice was process shape:

| Path         | What you build                           | What you still rebuild for “real” multi-client |
| ------------ | ---------------------------------------- | ---------------------------------------------- |
| Option 2     | Core in TUI process + optional HTTP bind | Process boundary, ensure/attach, quit ≠ kill   |
| **Option 4** | Core as daemon; TUI attaches             | Mostly polish                                  |

Option 2’s Phase A (“core always on, TUI as client”) is most of option 4. Doing 2 then 4 means extracting ownership twice in spirit (in-process client, then out-of-process client). **Go straight to daemon + attach.**

Cafe story still works — better, even:

1. Agent coding (daemon + TUI attached).
2. Leave cafe; stay-awake keeps machine on.
3. `/remote` opens LAN door (daemon already running).
4. Phone continues mid-stream.
5. Lid down; **daemon keeps running** even if TUI later detaches.
6. `/remote` → Stop closes the door; `crabcode stop` kills the brain.

---

## 2. Product model (one sentence)

> **One machine-wide daemon owns every workspace and agent runtime for the user. Every UI is a client. `/remote` only publishes the already-local API.**

```
crabcode
  → ensure daemon (start if needed, local bind)
  → attach TUI

crabcode serve [options]     # explicitly run the daemon headlessly/foreground
crabcode stop                # stop the discovered local daemon
crabcode attach <url>        # TUI client to an explicit local/remote host
```

OpenCode v2 parallel: `Service.ensure()` / `discover()` / `stop()`; TUI and browser are clients of the same HTTP API.

---

## 3. Process & network topology

```
┌──────────────────────────────────────────────────────────┐
│  crabcode daemon (one per user/machine)                  │
│                                                          │
│  • sessions, agent runtime, tools, permissions           │
│  • session event log (seq, multi-subscriber, resume)     │
│  • local HTTP API  (always)     127.0.0.1:<port>         │
│  • remote bind     (optional)   LAN / Tailscale + pair   │
└──────────────────────────────────────────────────────────┘
         ▲                    ▲                    ▲
         │                    │                    │
    TUI attach          browser / IDE         phone browser
    (default)           (local or remote)     (only if remote on)
```

| Surface                       | When                                     |
| ----------------------------- | ---------------------------------------- |
| Local API (`127.0.0.1`)       | Always while daemon lives                |
| Remote bind (LAN / Tailscale) | Only after `/remote` (or CLI equivalent) |
| Pair / pin                    | Required for non-localhost clients       |

### Lifecycle

| Action                       | Effect                                        |
| ---------------------------- | --------------------------------------------- |
| `crabcode`                   | `ensure` machine daemon → attach TUI          |
| `crabcode serve`             | Explicitly run the same daemon headlessly in the foreground |
| `crabcode stop`              | Gracefully stop the local daemon and its runs |
| quit TUI                     | **Detach client**; daemon keeps running        |
| `/remote`                    | Publish (bind + pair UI)                      |
| `/remote` while live         | Status dialog (URL, pair, clients) + **Stop** |
| Stop remote                  | Unpublish; daemon + sessions continue         |
| daemon shutdown              | Kill backend; all clients drop                |

**No special confirm on TUI quit** (locked). Detaching is normal; killing the daemon is the separate `crabcode stop` action. The daemon does not auto-exit with its last client and has no idle timeout in v1.

### CLI lifecycle contract (locked)

| Command | Contract |
|---|---|
| `crabcode` | Discover and health-check the registered local daemon. If absent/stale, launch the same `serve` implementation as a detached service; then attach a TUI. |
| `crabcode serve [options]` | Run the machine daemon explicitly, headless and in the foreground. Register it for local discovery. `Ctrl-C` performs graceful shutdown and removes its registration. Useful for VPS, service managers, logs, and debugging. |
| `crabcode stop` | Discover the registered **local** daemon, verify its identity, request authenticated graceful shutdown, wait, then safely escalate if necessary. Remove stale registration. |
| `crabcode attach <url>` | Attach a TUI to the exact supplied local or remote host. It does not perform local `ensure` and does not own that host's lifecycle. |

Only one machine daemon may own the normal local registration. If `crabcode serve` finds a healthy registered daemon, it exits with an actionable “already running” message instead of starting a competing core. A separately configured development/test instance must use an isolated state directory and explicit port.

`crabcode stop` means **stop the local backend**, not cancel the active turn. Turn cancellation remains a session-scoped UI/API action. V1 intentionally does not provide `crabcode stop <remote-url>`; remote daemon shutdown requires access on that host.

---

## 4. What `/remote` becomes

Not: start server, re-exec, or special process mode.  
**Yes: network policy on a living daemon.**

```
/remote off → on:  bind non-local + show pair/URL
/remote on:        status dialog + Stop
Stop:              drop remote bind / revoke pair
```

Defaults: reuse current `serve` bind/pair behavior; document Tailscale / private net; no public-internet goal.

Re-exec path for in-TUI `/remote` is **removed**. Headless `crabcode serve` remains for hosts without a TUI.

---

## 5. Locked product decisions (from cafe grill)

| Topic                 | Decision                                              |
| --------------------- | ----------------------------------------------------- |
| Dual control          | TUI and phone both drive (prompts, approve, cancel)   |
| Mid-stream remote     | Required; phone joins live turn                       |
| TUI while remote live | Full TUI + remote-live badge / status                 |
| Commands              | `/remote` only; live → status + Stop (no `/unremote`) |
| Session scope         | All host sessions                                     |
| Bind & security       | Current serve defaults                                |
| Failures              | Fail closed + clear error; no half-live remote        |
| Quit TUI              | Detach; no special confirm; daemon stays alive         |
| Daemon scope          | One per local user/machine; owns all workspaces        |
| Daemon lifetime       | Stays alive until `crabcode stop`, shutdown, or crash  |
| Client views          | Independent navigation/drafts; shared runtime/actions  |

---

## 6. Stream model — resumable multi-client (no Redis)

### Problem today

- Single-consumer `mpsc` of `ChunkMessage` drained only by the TUI.
- Remote leans on state poll / coarse SSE — weak multi-subscriber resume.

### Target: session event log

Redis Streams’ _properties_, not Redis:

```
agent / tools
    → append Event { seq, session_id, kind, payload, ts }
    → notify live subscribers
client (re)connect
    → GET /events?session=&after=seq   (or Last-Event-ID)
    → replay gap, then follow
```

| Layer                           | Role                                                                     |
| ------------------------------- | ------------------------------------------------------------------------ |
| In-process append log + fan-out | Hot path (always)                                                        |
| SQLite (existing state DB)      | Durable sessions; optional event/cursor durability across daemon restart |
| `tokio::sync::broadcast` alone  | Insufficient (no late join / resume)                                     |
| Redis / Valkey / Docker         | **Out of scope** for local daemon; only if multi-host cloud later        |

**Do not** ship Redis for laptop `crabcode`. One writer, N local/network readers — a small seq log is enough.

This is what makes “backend for any frontend” real: TUI, browser, phone, future IDE all tail the same log.

---

## 7. Client model

Every client is the same class of thing:

| Client                  | Transport                                    | Notes                  |
| ----------------------- | -------------------------------------------- | ---------------------- |
| Default TUI             | HTTP(+SSE/WS) to local daemon after `ensure` | Not “App owns runtime” |
| `crabcode attach <url>` | Same protocol to remote/local URL            |                        |
| Phone / browser         | Same API; only if remote bind + pair         |                        |
| `crabcode -p --attach`  | Non-interactive client of same API           | Later polish           |

**Avoid:** TUI mutating core via free `App` fields while HTTP uses another path.  
**Prefer:** one command/event protocol; TUI is a rich client of it (may use efficient local transport, but same semantics as HTTP).

OpenCode-shaped: server is the product; UIs are clients.

### Shared backend state vs client-local view state

Treat the daemon as a multi-client AI backend, not as a mirrored terminal.

| Shared through the daemon | Local to each client |
|---|---|
| Workspaces, sessions, transcripts | Selected workspace/session/tab |
| Live agent and tool output | Unsubmitted composer draft |
| Submitted and queued prompts | Cursor, selection, and focus |
| Pending approvals/questions | Scroll position and viewport |
| Approval/question results | Open panels, dialogs, sidebar state |
| Run status and cancellation | Local navigation history |
| Session create/archive/metadata | Other presentation preferences |

Clients may view different sessions concurrently. A submitted mutation is serialized by the daemon and becomes visible to every relevant client. Approval/question resolution is atomic: the first valid response wins; racing clients receive an already-resolved/conflict result. The daemon must never model a single global “currently selected session.”

---

## 8. Daemon ensure / discover (OpenCode-like)

Sketch (names flexible):

```text
Service.discover()  → healthy endpoint or none
Service.ensure()    → discover or start daemon, return endpoint
Service.stop()      → stop registered instance
Service.headers()   → auth for client
```

Registration: small file under state dir (e.g. XDG state `crabcode/service.json`) with URL, PID, version/protocol version, and local client credential. Because the daemon is machine-wide, discovery is not keyed by workspace.

**Scope (locked): one daemon per local user/machine.** It manages multiple workspace roots and all sessions visible on the host. Starting `crabcode` in a directory attaches to that daemon and opens/selects that workspace **for the new client only**; it does not navigate other attached clients.

Startup UX:

```text
# first run in project
$ crabcode
# starts daemon on 127.0.0.1:…. attaches TUI

# second terminal / second TUI
$ crabcode
# discovers same daemon, second attach

# headless host
$ crabcode serve

# explicit local service shutdown
$ crabcode stop
```

`crabcode stop` discovers the registered local daemon, requests authenticated graceful shutdown, waits for exit, and removes stale registration state. Its implementation uses the internal `Service.stop()` lifecycle API. It does not mean “stop the active agent turn”; turn cancellation remains a session command/UI action.

---

## 9. Relationship to other plans

| Doc                        | Role                                                                   |
| -------------------------- | ---------------------------------------------------------------------- |
| **This file**              | Architecture + ship plan for option 4                                  |
| `__remote-usage-plan.md`   | Product copy, Tailscale, attach/serve UX, safety                       |
| `INPROCESS_REMOTE_CAFE.md` | Cafe story + locked UX decisions; **shipping path superseded by this** |
| Existing `src/remote/`     | Evolve into daemon HTTP adapter; kill re-exec launch                   |

---

## 10. Non-goals (v1)

- Multi-user / team auth and ACLs
- Public internet exposure as a supported mode
- Redis/NATS/Kafka stream backend
- Docker required for local use
- Full cloud multi-tenant crabcode
- Per-workspace daemon isolation (the v1 service is machine-wide)
- Idle/last-client auto-shutdown (v1 stays up until explicit stop)

---

## 11. Ship phases

### Phase 0 — Spike (short)

Prove on a branch:

1. Minimal machine-wide daemon process + registration file + `ensure`.
2. TUI attaches over local HTTP (even if API is tiny: health + one session list).
3. Kill TUI; daemon still up; re-attach.
4. `crabcode stop` gracefully stops it and cleans registration.

**Exit:** confidence on process model before large extract.

### Phase 1 — Core extract + event log

1. Extract agent/session runtime out of `App` into shared core types.
2. Replace single-consumer stream drain with **session event log + multi-subscriber**.
3. TUI consumes via client API (local HTTP or in-process shim with **same** semantics — prefer real HTTP early to avoid a second extract).
4. `crabcode serve` becomes “run daemon in foreground” (or `--service` background) on that core.

**Exit:** single TUI user cannot tell (behavior parity); streams are multi-subscriber-ready.

### Phase 2 — Default ensure + attach

1. `crabcode` → `ensure` + attach (OpenCode-like).
2. Remove “TUI process owns the only runtime.”
3. Second attach works (second terminal).
4. Quit TUI = detach; daemon remains until `crabcode stop`.
5. Separate client navigation from shared runtime state; different attached TUIs may view different sessions.

**Exit:** every normal run is already the backend.

### Phase 3 — `/remote` as publish

1. `/remote` only toggles remote bind + pair on the daemon (no re-exec).
2. Status dialog + Stop; remote-live badge.
3. Phone joins mid-stream via event log resume.
4. Dual control as locked.
5. Fail closed on bind errors.

**Exit:** cafe story complete.

### Phase 4 — Polish (can overlap)

- `attach <url>`, `-p --attach`
- Pair UX, Tailscale docs (from `__remote-usage-plan.md`)
- Host status output parity with “ten-out-of-ten” host banner
- Daemon version skew / upgrade behavior

---

## 12. Implementation constraints (from current code)

Awareness for implementers (explore snapshot):

- Streaming: `ChunkMessage` over single `mpsc`; TUI `process_streaming_chunks` is the sole consumer (`App` / session stream state).
- Remote: HTTP routes under `src/remote/`; `/api/events` is coarse state streaming, not a seq log.
- Launch today: `/remote` → `running = false` → re-exec `serve` via `RemoteLaunchRequest` — **delete this path** for TUI remote.

Do not layer multi-client on top of exclusive `mpsc` without an event log.

---

## 13. Success criteria

| Criterion         | Measure                                                                      |
| ----------------- | ---------------------------------------------------------------------------- |
| Ensure/attach     | Two TUIs can attach to the one machine daemon and view different sessions    |
| Multi-workspace   | One daemon exposes all host workspaces/sessions without global navigation    |
| Detach            | Quit TUI does not kill the daemon or an in-flight agent                       |
| Explicit stop     | `crabcode stop` shuts down daemon/runs and cleans registration                |
| Mid-stream remote | `/remote` during generation; phone sees live turn                            |
| Resume            | Client reconnect with `after=seq` does not lose the turn                     |
| No Redis          | Local default has zero extra system services                                 |
| `/remote` meaning | Bind/unpublish only; daemon lifecycle separate                               |
| Re-exec gone      | In-TUI `/remote` never spawns a replacement process                          |

---

## 14. Risks & mitigations

| Risk                               | Mitigation                                                   |
| ---------------------------------- | ------------------------------------------------------------ |
| Extract too big; TUI parity breaks | Phase 0 spike; Phase 1 feature-flag dual path if needed      |
| Daemon zombie processes            | Registration pid check; `ensure` health; clear `stop`        |
| Port conflicts                     | Same fail-closed philosophy as cafe plan; clear errors       |
| Auth holes on LAN                  | Pair required off-localhost; doc Tailscale; no public mode   |
| Over-building cloud                | Event log in-process only; no Redis until multi-host product |

---

## 15. Decision log (option 4 specific)

- **Ship option 4 now** — skip option 2 as a product milestone.
- **`/remote` = publish** — bind LAN/pair; not start brain.
- **`crabcode` = ensure + attach** — always backend-shaped.
- **Event log, not Redis** — seq + multi-subscriber; SQLite optional durability.
- **TUI is a client** — same semantic protocol as phone/browser.
- **Cafe UX decisions** — carry over from `INPROCESS_REMOTE_CAFE.md` grill.
- **Machine-wide scope** — one daemon per local user, all workspaces/sessions.
- **Explicit lifetime** — no idle exit; `crabcode stop` ends the service.
- **Independent views** — shared runtime/actions, client-local navigation and drafts.

---

## 16. Next action

Start **Phase 0 spike**: daemon registration + ensure + attach health, before large `App` extract.
