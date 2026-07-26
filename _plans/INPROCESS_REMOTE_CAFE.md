# Cafe `/remote` UX decisions

Internal planning. Complements `__remote-usage-plan.md` (product/remote usage).

**Shipping architecture:** [`OPTION4_DAEMON_BACKEND.md`](./OPTION4_DAEMON_BACKEND.md)  
This file only keeps the **cafe story + locked UX decisions**. Option 2 (in-process) was explored then skipped as a product milestone.

Last updated: 2026-07-25.  
Status: **UX decisions locked**; implement under option 4.

---

## Product story

1. Agent is still coding in the TUI (daemon + attach).
2. User needs to leave the cafe; stay-awake keeps the machine on.
3. User runs `/remote`, sees the dialog, pairs from phone.
4. Same live session continues on the phone **including mid-stream**.
5. Laptop lid closes; **daemon keeps running**.
6. Later: `/remote` → status dialog → **Stop** (unpublish); or quit TUI (detach).

Under option 4, `/remote` means **publish the already-running local API**, not start a server.

---

## Target shape

```
daemon (core + event log + local API)
  ├── TUI attach
  └── /remote → optional LAN bind → phone
```

```
/remote (off → on):  publish bind + pair dialog
/remote (on):        status dialog (URL, pair, clients) + Stop
Stop / fail:         unpublish; daemon continues
quit TUI:            detach client (daemon may keep running)
```

---

## Decisions (locked)

| Topic | Decision |
|---|---|
| Dual control | Either TUI or phone can drive (prompts, approve, cancel). |
| Mid-stream `/remote` | Required. Phone joins live turn. |
| TUI after remote | Full TUI + “remote live” badge / URL·pair status. |
| Command UX | `/remote` only. Live → status dialog + Stop. No `/unremote`. |
| Session scope | All host sessions (multi-session). |
| Bind & security | Reuse current `serve` defaults (LAN + pair). Doc private-net/Tailscale. |
| Failures | Fail closed + clear error; remote off; retry via `/remote`. |
| Quit TUI | Detach; no special confirm. Daemon lifecycle separate. |
| Old re-exec | Kill re-exec for in-TUI `/remote`. Keep `crabcode serve` as the explicit headless/foreground daemon command. |
| Architecture | Option 4 daemon + ensure/attach — see `OPTION4_DAEMON_BACKEND.md`. |

---

## Architecture ladder (historical)

| # | Approach | Status |
|---|---|---|
| 0 | `/remote` → quit TUI → re-exec `serve` | today (remove) |
| 2 | In-process HTTP + TUI host | skipped as product milestone |
| 3 | Mode switch / detach TUI, same process | later if needed |
| **4** | Always-on daemon; TUI + phone are clients | **chosen** |

---

## Decisions log (chronological)

- Dual control: shared (either drives).
- Mid-stream `/remote`: required, join live turn.
- TUI after remote: full TUI + status badge.
- Command UX: `/remote` only; live → status + Stop.
- Session scope: all host sessions.
- Bind & security: current serve defaults.
- Failure modes: fail closed + clear error.
- Old re-exec path: kill for `/remote`.
- Live dialog: status + Stop.
- **Ship path: option 4 now** (not option 2).
