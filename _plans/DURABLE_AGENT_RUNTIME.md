# Agent Runtime Capability Roadmap

Status: **RFC / umbrella roadmap**

This document does not replace implementation plans that already exist. It defines the product-level capability gaps that remain after those plans land, assigns ownership between plans, and establishes an implementation sequence that avoids parallel sources of truth.

The intended outcome is to evolve Crabcode from an interactive coding agent into a local-first agent runtime whose work is inspectable, resumable, permissioned, verifiable, and reversible.

## 1. Scope and authority

This RFC is authoritative only for:

- cross-plan integration contracts;
- capability ownership where no focused plan exists;
- dependency order between focused plans;
- the definition of new agent-runtime capabilities;
- criteria for splitting follow-up implementation plans.

This RFC is not the source of truth for daemon topology, workspace/session UX, provider completion semantics, base permission behavior, or generation timing primitives.

If this RFC conflicts with a focused plan listed below, the focused plan wins until the conflict is explicitly resolved in both documents.

## 2. Existing sources of truth

| Area | Source of truth | This RFC's role |
| --- | --- | --- |
| Daemon process topology, attach/detach, machine-wide runtime, resumable multi-client stream | `OPTION4_DAEMON_BACKEND.md` | Consume the runtime and event-log contracts; do not redefine them. |
| Workspace/session grouping, background sessions, client-local view state, durable permission/question waits | `MULTIWORKSPACE.md` | Add agent-run concepts without replacing workspace/session UX. |
| T0/T1/Tn, TTFT, decode TPS, and output-token persistence | `metrics-plan.md` | Reuse these primitives as provider-span metrics. |
| Allow/ask/deny rules, permission interceptor, approval UX, path classification | `TOOL_SYSTEM_PERMISSIONS.md` | Extend policy inputs with provenance and execution profiles; do not create a second permission engine. |
| Provider/tool-loop completion and premature-completion bug fixes | `PREMATURE_COMPLETE_BUG.md` | Keep provider-turn completion semantics independent from run-level acceptance. |
| TUI decomposition and reducing `App` responsibilities | `REFACTORING.md` | Align service boundaries; do not prescribe a second UI refactor. |
| Per-repository session scope | `TODO_PER_PROJECT_SESSIONMEMORY.md` | Treat repository identity as workspace/session scope, not semantic memory. |
| Existing task tool and subagent behavior | `OC_TOOL_SYSTEM_PRD.md` and current implementation | Extend lifecycle control without replacing current tool contracts prematurely. |

Related historical plans may contain useful context, but they are not automatically normative. Any implementation PR should cite the focused plan whose contract it changes.

## 3. Architectural diagnosis

Crabcode already has strong foundations:

- multi-provider streaming and structured tool loops;
- a tool registry with centralized permission interception;
- delegated subagents and parent/child sessions;
- persisted sessions and structured message parts;
- interactive questions and approvals;
- project instruction discovery;
- ACP and remote surfaces;
- basic generation timing and token metadata.

The remaining gap is a control plane above these pieces. Today, orchestration is spread across `App`, session state, provider loops, tool handlers, and in-memory channels. A session transcript can survive restart, but it is not a complete representation of an executing goal.

The target is not a second chat architecture. It is a thin agent-runtime layer that composes the existing daemon, session, provider, tool, permission, and UI systems.

```text
TUI / Remote / ACP / CLI
          │
          ▼
Existing daemon and session runtime
          │
          ▼
Agent-runtime capabilities
├── run identity and lineage
├── subagent lifecycle control
├── goal contracts and verification
├── workspace memory and context planning
├── provenance, execution profiles, and redaction
├── workspace checkpoints
└── tracing, replay, evaluation, and scheduling
```

## 4. Non-negotiable boundaries

### 4.1 Session, turn, and run are different concepts

- A **session** is the user-visible conversation container owned by the workspace/session architecture.
- A **turn** is one provider/tool-loop interaction within a session.
- A **run** is an optional goal-oriented execution that may span turns, child agents, approvals, verification, and artifacts.

A simple chat prompt does not need a heavy workflow object. The initial implementation may assign a lightweight run identity to every turn while enabling richer run state only when needed.

### 4.2 Provider completion is not run acceptance

Provider and tool-loop completion remains governed by the structured lifecycle described in `PREMATURE_COMPLETE_BUG.md` and the AISDK implementation.

A run-level goal contract is an additional, opt-in acceptance gate:

```text
provider turn completes
        │
        ▼
run coordinator evaluates remaining work
        ├── acceptance met → run succeeds
        ├── more work needed → start another turn/step
        └── blocked → wait for user or fail explicitly
```

`update_plan` must not become a hidden source of provider completion truth. Plan state may inform the coordinator, but structured provider events remain authoritative for whether a turn has ended.

### 4.3 One event stream, not competing ledgers

`OPTION4_DAEMON_BACKEND.md` and `MULTIWORKSPACE.md` already establish resumable session events. Agent-runtime events must extend or reference that stream rather than introduce an unrelated transport or replay log.

Run events may have their own persistence representation, but they must share stable session/generation identities and map deterministically into client-visible events.

### 4.4 One permission engine

Provenance, sandbox profiles, and secret access become inputs to the existing allow/ask/deny evaluator. They must not bypass or duplicate `TOOL_SYSTEM_PERMISSIONS.md`.

### 4.5 `App` remains a client-facing projection

Runtime extraction should follow the daemon and refactoring plans. This RFC requires only that new capabilities live behind services or commands that can be called by TUI, remote, ACP, and headless clients consistently.

## 5. Capability ownership map

| Capability | Status | Owner |
| --- | --- | --- |
| Durable daemon and multi-client event replay | Already planned | `OPTION4_DAEMON_BACKEND.md` |
| Background workspace/session execution | Already planned | `MULTIWORKSPACE.md` |
| Durable approval/question waits | Already planned | `MULTIWORKSPACE.md` + `TOOL_SYSTEM_PERMISSIONS.md` |
| Provider timing primitives | Already planned | `metrics-plan.md` |
| Provider-turn completion correctness | Existing focused bug plan | `PREMATURE_COMPLETE_BUG.md` |
| Stable run/step/agent/invocation lineage | New integration capability | This RFC; split into focused plan before implementation |
| Controllable subagent lifecycle | New capability | This RFC; split into focused plan |
| Goal contracts and verification | New capability | This RFC; split into focused plan |
| Typed workspace memory and context planner | New capability | This RFC; split into focused plan |
| Provenance, redaction, and execution profiles | Partially new | This RFC extending `TOOL_SYSTEM_PERMISSIONS.md` |
| Run-scoped workspace checkpoints | New capability | This RFC; split into focused plan |
| Unified traces, cost, replay, and evals | Partially new | This RFC extending `metrics-plan.md` |
| Scheduler and external triggers | Deferred capability | This RFC after durable runtime lands |

## 6. Integration foundation: stable runtime lineage

Priority: **P0**, but implementation must align with the daemon/session work.

This is the minimum shared contract required by every new capability. It does not define a second daemon or a second session event log.

### 6.1 Required identities

Introduce typed, stable identifiers where missing:

```text
run_id
step_id
agent_run_id
invocation_id
trace_id
```

They must compose with existing identities:

```text
workspace_id
session_id
generation_id
message_id
tool_call_id
client_id
```

### 6.2 Correlation requirements

Every provider request, tool invocation, approval, question, child agent, artifact, and verification result should be attributable to:

- workspace and session;
- generation/turn;
- run and step when applicable;
- parent agent run;
- trace/span lineage.

The first milestone does not require full event sourcing. It requires enough correlation that runtime behavior can later be persisted and replayed without changing identifiers.

### 6.3 Integration acceptance criteria

- Existing interactive behavior remains unchanged.
- Logs can distinguish concurrent parent and child work.
- Remote and TUI events use the same correlation identifiers.
- No new persistence layer competes with the daemon/session event stream.
- IDs survive restart when their owning runtime object is durable.

## 7. New capability A: subagent control plane

Priority: **P0 after lineage foundation**.

### 7.1 Gap

The current task flow supports real delegated work, but the parent interaction is close to synchronous delegation: invoke a task and receive one final result. Parent agents lack first-class lifecycle operations for long-running or parallel children.

### 7.2 Required operations

A focused plan should define stable operations equivalent to:

```text
spawn_agent
after spawn: inspect status
wait_agents
send_agent_message
cancel_agent
retry_agent
close_agent
```

The exact tool names are not fixed by this RFC. The runtime contract is.

### 7.3 Required behavior

- Child agents have stable `agent_run_id` values.
- Parent/child lineage survives UI navigation and reconnect.
- Detached children continue under the daemon/session runtime.
- Parent cancellation policy is explicit: cancel children, detach children, or ask.
- Partial progress and terminal outcomes are inspectable.
- Concurrency and token/cost budgets are enforced centrally.
- A child cannot escalate tool permissions beyond the effective parent/run policy.

### 7.4 Orchestration patterns enabled later

- fan-out/fan-in research;
- researcher → implementer → reviewer;
- parallel work partitioned by file ownership;
- critic or red-team loops;
- map/reduce over repositories or artifacts.

These patterns should be built on the lifecycle API, not encoded as special loops in `App` or separate tools.

### 7.5 Acceptance criteria

- A parent can spawn two children, inspect both, and wait for either/all.
- A user can cancel one child without cancelling unrelated work.
- Reopening a session shows the correct child state.
- Duplicate child execution is prevented after reconnect or retry.
- Child tool calls retain complete trace and permission lineage.

## 8. New capability B: goal contracts and verification

Priority: **P0/P1 after stable run identity**.

### 8.1 Gap

A model can end a valid provider turn without proving that the user's broader task is complete. Plans are currently useful UI/model coordination signals but are not durable executable workflows with acceptance criteria.

### 8.2 Goal contract

A run may optionally declare:

```yaml
goal: "Implement automatic session compaction"
acceptance:
  - command: "cargo test session::compaction"
  - artifact: "implementation_summary"
  - condition: "no unresolved tool or permission waits"
completion_policy:
  require_all: true
  verifier: "deterministic_then_agent"
```

The schema above is illustrative. A focused plan must define the final representation and compatibility path.

### 8.3 Verification order

1. Deterministic state checks.
2. Deterministic commands/tests.
3. Artifact presence and integrity.
4. Agent judgment only for criteria that cannot be evaluated deterministically.
5. Explicit user review when policy requires it.

### 8.4 Relationship with `update_plan`

`update_plan` remains a planning/progress interface. It may later gain stable step IDs and dependency metadata, but:

- it does not determine whether a provider response is final;
- an omitted plan does not make a turn invalid;
- plan completion alone does not prove acceptance;
- run verification can operate without exposing a model-authored plan.

### 8.5 Acceptance criteria

- A provider turn may complete while the run remains `verifying` or `blocked`.
- Deterministic failures cannot be overridden silently by a model completion message.
- Verification output is linked to the run and visible to clients.
- Manual and automatic runs use the same acceptance model.

## 9. New capability C: workspace memory and context planner

Priority: **P1**.

This capability is separate from repository-scoped session listing in `TODO_PER_PROJECT_SESSIONMEMORY.md`.

### 9.1 Gap

Current persistence provides transcripts, compaction summaries, project instructions, and prompt history. It does not provide curated cross-session knowledge with authority, provenance, correction, deletion, and retrieval controls.

### 9.2 Typed records

Do not persist every chat message as permanent memory. Begin with explicit or high-confidence records:

```text
Decision
Constraint
ProjectFact
UserPreference
FailedApproach
SuccessfulPattern
PendingWork
ArchitectureNote
```

Each record needs:

- workspace/global scope;
- source and timestamp;
- explicit vs inferred origin;
- confidence;
- authority level;
- lifecycle status;
- correction/deletion history.

### 9.3 Authority order

Context assembly must preserve an explicit precedence model:

```text
system policy
> project instructions
> explicit user preferences
> confirmed workspace memory
> inferred workspace memory
> retrieved untrusted content
```

Memory must never silently override project instructions or a current user request.

### 9.4 Retrieval approach

Start with:

1. exact/structured filters;
2. SQLite FTS lexical retrieval;
3. recency, confidence, and authority ranking;
4. token-budget selection;
5. provenance-visible prompt injection.

Embeddings are deferred until retrieval quality can be measured against an evaluation set.

### 9.5 Context planner

Before provider execution, calculate a model-aware budget for:

- system and project instructions;
- tool schemas;
- recent conversation;
- active goal/plan state;
- retrieved memory;
- historical artifacts;
- reserved output and reasoning space.

The planner should prune low-value tool noise and trigger compaction before an avoidable context overflow. Exact tokenizer support should replace rough character heuristics where provider tokenizers are available.

### 9.6 Operator controls

Users must be able to inspect, correct, forget, and disable memory. Final command names are deferred to the focused plan.

### 9.7 Acceptance criteria

- Every injected memory item has visible provenance.
- Users can remove or correct an item permanently.
- Inferred memory cannot silently become authoritative.
- Retrieval improves a benchmark corpus before semantic retrieval is enabled by default.
- Raw transcript retention and memory retention are independently configurable.

## 10. New capability D: provenance, redaction, and execution profiles

Priority: **P0 for redaction; P1 for provenance and sandboxing**.

### 10.1 Ownership boundary

`TOOL_SYSTEM_PERMISSIONS.md` owns allow/ask/deny decisions and approval UX. This capability adds richer inputs and enforcement environments to that evaluator.

### 10.2 Central redaction

Before expanding trace/event persistence, introduce one redaction layer used by:

- provider request/response logging;
- tool arguments and outputs;
- command output;
- remote event payloads;
- trace exports;
- error reports.

Redaction targets include authorization headers, API keys, cookies, configured secret environment variables, and known credential fields. Redaction must happen before persistence or remote delivery, not only when rendering.

### 10.3 Provenance labels

Context and action causes should be distinguishable as:

```text
trusted_system
trusted_project_rule
user_input
local_source
web_content
command_output
subagent_output
retrieved_memory
```

The policy evaluator may require elevated approval when untrusted content materially causes a sensitive action.

### 10.4 Execution profiles

Define policy-backed profiles such as:

```text
read_only
workspace_write
networked
privileged
host_unrestricted
```

Profiles should constrain filesystem, environment, secrets, network access, process/time/output resources, and allowed tool categories. Platform-specific sandbox implementation is deferred to a focused design.

### 10.5 Acceptance criteria

- Sensitive values do not enter persisted logs or remote payloads in safety tests.
- Provenance can be traced from an external input to a requested sensitive action.
- Execution profiles feed the existing permission evaluator.
- Child agents cannot gain a more privileged profile without explicit policy approval.
- Unsupported sandbox guarantees are reported honestly rather than simulated.

## 11. New capability E: run-scoped workspace checkpoints

Priority: **P1**.

### 11.1 Gap

Individual mutation tools can protect against stale writes, but a complete run may modify many files through file tools and shell commands. There is no unified run-level inspection and recovery boundary.

### 11.2 Required behavior

Before the first mutating action in a protected run, capture enough state to provide:

- changed-path inventory;
- before/after hashes;
- reversible patches where possible;
- untracked-file tracking;
- attribution to run, step, and invocation;
- conflict detection when the workspace diverges later.

Git repositories may use git object/tree primitives without forcing commits. Non-git workspaces need a bounded patch journal or copy-on-write strategy.

### 11.3 Limits

A checkpoint cannot guarantee rollback of arbitrary external side effects, including network APIs, package registries, database changes, or destructive shell commands outside the captured workspace. Such actions require policy and explicit audit records.

### 11.4 Acceptance criteria

- Users can inspect a run-level diff.
- File-tool and shell-originated workspace mutations are attributed where technically possible.
- Rollback refuses to overwrite conflicting later edits silently.
- Dirty worktrees are supported without creating hidden commits.
- External side effects are marked non-reversible.

## 12. New capability F: unified tracing, cost, replay, and evaluation

Priority: **P1**, built on lineage and redaction.

### 12.1 Ownership boundary

`metrics-plan.md` remains authoritative for generation timing primitives. This capability organizes those metrics into end-to-end runtime traces and adds normalized usage, cost, replay, and quality evaluation.

### 12.2 Trace hierarchy

```text
run span
├── turn/provider span
│   ├── retry span
│   ├── tool invocation span
│   ├── approval/question wait span
│   └── child agent span
└── verification span
```

A trace should record, where available:

- provider/model;
- T0/T1/Tn and usage metadata;
- retries and finish reasons;
- tool outcome and duration;
- wait duration;
- child lineage;
- cancellation/failure reason;
- normalized estimated cost;
- redacted payload hashes or artifact references.

Provider-reported usage should be preserved separately from estimates.

### 12.3 Replay modes

A focused plan should distinguish:

- recorded-provider playback for UI/runtime debugging;
- mocked-tool replay for deterministic tests;
- dry-run policy evaluation;
- approximate rerun from a selected step.

Replay must never imply deterministic reproduction of model output. Non-idempotent external actions must not execute automatically during replay.

### 12.4 Evaluation layers

Build evaluation coverage incrementally:

1. deterministic protocol/unit fixtures;
2. recorded provider-stream fixtures;
3. tool-use scenarios;
4. repository coding tasks;
5. crash/reconnect cases;
6. multi-agent coordination cases;
7. prompt-injection and secret-exfiltration cases;
8. checkpoint/rollback cases;
9. false-completion and verification cases.

### 12.5 Acceptance criteria

- A run can be inspected across provider, tool, approval, and child-agent boundaries.
- Cost distinguishes reported usage from estimates.
- Replay cannot repeat an external mutation without explicit policy.
- CI can execute a stable scenario corpus and track regressions.
- Trace retention and export respect redaction policy.

## 13. Deferred capability: scheduler and triggers

Priority: **P2 after durable daemon execution, policy, and verification**.

Scheduled or event-triggered agents should reuse the same run coordinator, permissions, execution profiles, verification, tracing, and checkpoint rules. There must not be a scheduler-specific agent loop.

Possible triggers include time/cron, repository changes, CI failures, issues, and manually queued background work. The exact trigger set is not part of the first implementation.

## 14. Dependency graph

```text
OPTION4 daemon + MULTIWORKSPACE runtime
                  │
                  ▼
         stable runtime lineage
          ┌───────┼────────┐
          ▼       ▼        ▼
   subagent CP  redaction  tracing base
          │       │        │
          ▼       ▼        ▼
  goal contracts policy   replay/evals
          │       │
          ├───────┼─────────────┐
          ▼       ▼             ▼
     checkpoints memory       scheduler
```

Parallel work is possible after shared identities and redaction contracts are stable. Scheduler work should not start before durable background execution and verification exist.

## 15. Recommended plan split

Do not implement this umbrella RFC as one PR or one giant implementation plan. After review, create focused plans in this order:

1. **Runtime lineage and trace identity**
   - identifiers, correlation rules, event integration, redaction prerequisites;
   - explicitly aligned with daemon/session schemas.

2. **Subagent control plane**
   - lifecycle operations, concurrency, cancellation, lineage, budgets.

3. **Goal contracts and verification**
   - acceptance schema, deterministic checks, plan relationship, UI states.

4. **Policy extensions and execution profiles**
   - provenance, central redaction, sandbox capability matrix;
   - amendments to `TOOL_SYSTEM_PERMISSIONS.md` where needed.

5. **Workspace checkpoints**
   - git/non-git capture, shell attribution, diff and rollback behavior.

6. **Workspace memory and context planning**
   - typed memory, authority, retrieval, controls, evaluation.

7. **Runtime tracing, cost, replay, and evals**
   - extends `metrics-plan.md`; may begin earlier with trace scaffolding.

8. **Scheduler and triggers**
   - only after the required runtime capabilities are proven.

Each focused plan must state which existing source-of-truth documents it amends and which it merely consumes.

## 16. Recommended first milestone

The first implementation milestone should be intentionally narrow:

> Add stable runtime lineage and centralized redaction to the existing single-agent flow, integrated with the daemon/session event model, without changing visible TUI behavior.

Deliverables:

1. typed run/step/agent/invocation/trace IDs;
2. correlation across provider requests, tools, permissions, questions, and subagents;
3. mapping into existing session/generation events;
4. centralized redaction before log, persistence, and remote delivery;
5. trace-friendly structured logs;
6. regression tests for concurrent parent/child attribution and secret leakage.

Explicitly excluded from milestone one:

- new daemon topology;
- executable plans;
- automatic run resume;
- sandbox workers;
- semantic memory;
- scheduler;
- broad TUI redesign.

This milestone creates the integration seam needed by later capability plans without reopening decisions already owned elsewhere.

## 17. Migration and compatibility rules

- Preserve current interactive chat behavior before enabling richer run semantics.
- Introduce optional fields and dual-read compatibility where persisted schemas change.
- Keep old sessions readable without synthetic historical trace data.
- Move one ownership boundary at a time; avoid a big-bang `App` rewrite.
- Gate operator-facing behavior changes behind focused feature flags when useful.
- Do not persist sensitive payloads first and promise to redact them later.
- Do not claim crash-safe resume until tool idempotency and ambiguous-operation behavior are specified.

## 18. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Umbrella RFC becomes a competing implementation spec | Keep focused plans authoritative and split new work before coding. |
| Runtime IDs diverge from daemon/session identities | Define correlation contract jointly in the first focused plan. |
| Goal verifier conflicts with provider completion | Preserve the turn/run boundary in section 4.2. |
| Provenance creates a second permission system | Feed provenance into the existing evaluator only. |
| Trace storage leaks credentials | Central redaction before persistence; adversarial leakage tests. |
| Resume or replay duplicates mutations | Classify idempotency; pause ambiguous operations; never auto-replay external effects. |
| Memory becomes an opaque instruction source | Typed records, authority rules, provenance, correction, deletion, opt-out. |
| Checkpoint promises exceed platform guarantees | Mark non-reversible effects and report unsupported guarantees honestly. |
| New services deepen architecture complexity | Require measurable removal of orchestration from client/UI code. |

## 19. Non-goals

This RFC does not propose:

- replacing the daemon plan;
- replacing the workspace/session model;
- replacing AISDK provider completion semantics;
- replacing the permission evaluator;
- making `update_plan` mandatory for every turn;
- storing every transcript message as permanent memory;
- deterministic replay of nondeterministic model output;
- silently replaying destructive or external actions;
- a cloud control plane or multi-host worker fleet in the initial roadmap;
- one PR that implements every capability in this document.

## 20. RFC acceptance checklist

Before this document is treated as an approved roadmap:

- [ ] Daemon/runtime maintainers agree that agent events extend the existing event model.
- [ ] Workspace/session maintainers agree on session/turn/run boundaries.
- [ ] AISDK maintainers agree that goal acceptance does not alter provider completion semantics.
- [ ] Permission maintainers agree that provenance and profiles are evaluator inputs, not a replacement engine.
- [ ] Metrics maintainers agree on how T0/T1/Tn map into trace spans.
- [ ] The first focused plan defines stable identities and redaction without broad product behavior changes.
- [ ] Each later capability is split into a reviewable implementation plan before code lands.

## 21. Final recommendation

Adopt this document as an umbrella RFC, not as an implementation specification.

The defensible product direction remains:

> Crabcode is a local-first, multi-provider agent runtime where work can be inspected, resumed, permissioned, verified, and reversed.

The next concrete action is not to implement the full roadmap. It is to review and approve the ownership boundaries, then create the focused **runtime lineage and redaction** plan that integrates with the already-planned daemon and workspace runtime.