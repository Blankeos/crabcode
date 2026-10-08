# Benchmarking

`just bench-agents` is the single coding-agent benchmark runner. Every agent uses
the same task registry, prompts, checks, and artifact format. Startup/idle performance
is measured separately with `just bench-perf`.

### Start here

| Want to change…                                | Open…                                                                         |
| ---------------------------------------------- | ----------------------------------------------------------------------------- |
| Which CLI runs, or its flags                   | `src/agents.ts`                                                               |
| A benchmark task                               | `src/tasks/` (`coding.ts` for smoke tasks, `workflow.ts` for the medium task) |
| How results are graded                         | `src/checks.ts`                                                               |
| What trace data is measured                    | `src/diagnostics.ts`                                                          |
| How attempts run and retry                     | `bench-agents.ts`                                                             |
| Workspace files, process cleanup, or artifacts | `src/workspace.ts`                                                            |
| The human-readable report                      | `src/report.ts`                                                               |

Files ending in `.test.ts` are offline regression tests, not additional benchmark
runners. `src/runner.test.ts` covers attempts, grading isolation, and process cleanup
with fake CLIs. `src/tasks/fixtures.test.ts` checks all five fixtures against broken,
known-good, and deliberately flawed solutions without calling a model. Diagnostics
tests keep detailed accounting checks; CLI and report tests focus on behavior rather
than wording or layout.

`src/llm/diagnostics.rs` **outside this directory** is the actual Crabcode print-mode
trace writer. The benchmark's `src/diagnostics.ts` only reads those traces and Claude
stream events. Keeping that distinction means benchmark code does not run inside
Crabcode or your print-mode integrations.

## Agents and defaults

The default agents remain **crabcode, opencode, codex, and grok-build**. Claude Code
is available as **`claude`**, but is **never included by default**. Adding its adapter
does not change the default task selection, per-task models, native tool surfaces,
or the default one-attempt policy.

```sh
just test-bench                    # Offline runner, adapter, grader, and report tests
just bench-agents --help
just bench-agents --list-tasks
just bench-agents --estimate       # Plan only; no model calls

# Existing multi-provider comparisons
just bench-agents --agents crabcode,codex --model openai/gpt-5.5 --tasks bugfix-js
just bench-agents --agents crabcode,grok-build --model grok-4.5 --tasks bugfix-js

# Claude opt-in on an existing task; its model flag affects only Claude
just bench-agents --agents claude --claude-model claude-sonnet-5-5 --tasks bugfix-js
just bench-agents --agents crabcode,claude --model '<provider>/claude-sonnet-5-5' \
  --claude-model claude-sonnet-5-5 --tasks bugfix-js --tool-profile coding

# Different models are allowed but are not a controlled harness-only experiment
just bench-agents --agents crabcode,claude --model openai/gpt-5.5 \
  --claude-model claude-sonnet-5-5 --tasks bugfix-js

# Medium multi-file task; trace the current optimized Crabcode build
cargo build --release
BENCH_CRABCODE_BIN=./target/release/crabcode just bench-agents \
  --agents crabcode,claude --model '<provider>/claude-sonnet-5-5' \
  --claude-model claude-sonnet-5-5 --claude-effort low --reasoning-effort low \
  --tasks workflow-runner --tool-profile coding --diagnostics \
  --runs 3 --max-attempts 2 --timeout-ms 300000 --keep

# Machine-readable output; Markdown and raw attempt artifacts are retained as well
just bench-agents --agents claude --claude-model claude-sonnet-5-5 \
  --tasks page-boundaries --out benchmark-reports/results.json \
  --report benchmark-reports/report.md --keep
```

Replace `<provider>` with the provider configured for Crabcode. Quote a complete
model reference or replace the placeholder before executing examples in a shell.
Global `--model` selects the model for other agents; **`--claude-model` selects only
Claude's model**. No provider is chosen automatically for Crabcode. Claude rejects
non-Claude model IDs rather than silently substituting a different model. A Claude
ID supplied through `--model` can be used directly, stripping one provider prefix.

Claude-only options:

- `--claude-model ID` / `BENCH_CLAUDE_MODEL`: independent model override.
- `--claude-effort LEVEL` / `BENCH_CLAUDE_EFFORT`: requested effort; default `low`.
- `--claude-tools LIST`: explicit tool-list override, recorded in the report.
- `BENCH_CLAUDE_BIN`: executable override; relative paths resolve from the repository.
- `BENCH_CLAUDE_CMD`: full command-template override. The default adapter uses
  safe mode, isolated settings/MCP, fresh conversations, and `stream-json` output.
  Overrides are responsible for preserving those behaviors and structured output.

Other binary/command overrides follow `BENCH_<AGENT>_BIN` and `BENCH_<AGENT>_CMD`,
with hyphens changed to underscores (`BENCH_GROK_BUILD_BIN`). Templates accept
`{model}`, `{prompt}`, `{repo}`, `{effort}`, and `{trace}`; values are shell-quoted.
Crabcode command overrides must include `--trace-jsonl {trace}` when diagnostics
are requested. `--reasoning-effort` overrides `BENCH_CRABCODE_REASONING` (default
`medium`); Claude's effort setting is separate.

## Tasks and grading

Existing tasks, model tiers, and tag/difficulty filters remain available. Defaults
use `openai/gpt-5.3-codex-spark` for smoke/medium tasks and `openai/gpt-5.5` for hard
tasks; a task's `model` or `--model` overrides that choice. These model defaults do
not apply to Claude when `--claude-model` is supplied.

Seven additional dependency-free fixtures are **opt-in** for all agents:

| ID                 | Difficulty | Behavior                                                            |
| ------------------ | ---------- | ------------------------------------------------------------------- |
| `page-boundaries`  | smoke      | Pagination and input-validation boundaries                          |
| `job-status`       | smoke      | Cross-file job-status addition                                      |
| `billing-refactor` | smoke      | Helper extraction while preserving charges                          |
| `noisy-redirects`  | smoke      | Nested repo navigation and redirect cycles                          |
| `workflow-runner`  | medium     | DAG validation, bounded async concurrency, and failure propagation  |
| `async-cache`      | hard       | Single-flight loading, TTL/LRU eviction, and invalidation races     |
| `json-patch`       | hard       | Atomic patch operations, pointer decoding, and prototype-safe trees |

Select them by ID or an explicit tag/difficulty filter. Leaving all filters absent
preserves the original default suite. They run for Claude, Crabcode, or any other
selected agent—not a special model-specific runner.

The opt-in fixtures use a clean-room grader: candidate source modules are copied
to a temporary directory and immutable original visible/hidden tests are supplied
by the supervisor. Candidate test edits, package scripts, Bun preload configuration,
and symlinks cannot alter those checks. The broken baseline must fail before a
trial starts. Reference and deliberately flawed solutions are checked offline.
The workflow fixture has three suites containing eleven cases; concurrency checks
use explicit promise gates rather than wall-clock sleep thresholds.
The hard cache fixture also uses an injected clock and promise gates. The JSON
Patch fixture specifies its supported subset explicitly, including root operations
and array-index rules. Their dedicated offline tests validate reference solutions
and reject targeted faulty implementations.

Existing tasks retain their task-specific `check` functions and localhost static
servers where needed. They are not automatically upgraded to clean-room grading.
For retries, prefer immutable checks: older helpers can inject hidden tests into a
candidate workspace during grading, which may reveal them to later attempts.

## Attempts and completion

- **First-attempt pass** means the first complete CLI invocation passes all checks.
  It can include many native model/tool/test iterations, including self-correction.
- **Final pass** allows supervisor-assisted repairs under `--max-attempts` (default
  `1`). A repair starts a fresh conversation in the same edited workspace, with the
  original task and cumulative failure output. It is not autonomous discovery and
  is not independent `pass@k` sampling.
- Held-out failure output is unavailable before the initial attempt, but is exposed
  when requesting repair. Do not conflate first-attempt and repaired scores.
- The timeout is shared agent-process time across attempts; each grader has its own
  bounded timeout. Existing per-task timeout overrides still apply.
- Agent order alternates per task/repetition. `--parallel` runs each agent group
  concurrently, but shared load and rate limits confound timing comparisons.

## Tools, configuration, and safety

`--tool-profile native` is the default and leaves each harness's native tool surface
unchanged. `--tool-profile coding` exposes six equivalent capabilities on Crabcode
and Claude: read, write, edit, shell, glob, and grep. It uses a fixture-local Build
agent allowlist for Crabcode and Claude's `--tools` list. Other agents retain native
tools; an explicit `--claude-tools` override also removes the matched-tools guarantee.
None of this changes normal Crabcode defaults or personal configurations.

Claude comparisons, opt-in graded fixtures, coding profiles, and diagnostic runs
isolate global XDG project config and global Claude instructions and give each
workspace a temporary git root. Existing saved authentication remains available;
there is no automatic login. Task-local model configuration and the full command
are saved with the run.

**Earlier comparison caveat:** initial reports gave Claude six tools but Crabcode
eighteen. Those timings included a tool-surface confound and cannot establish a
pure harness latency gap. Preserve model, tools, prompt, budgets, and task hashes
when comparing changes.

Workspaces are disposable but **not an OS security sandbox**. Candidate source and
tools execute locally. Use trusted fixtures on a workstation, and a disposable
machine/container for untrusted tasks. Unattended permission settings allow source
editing and shell execution. Ctrl+C stops process groups and saves partial results.

## Diagnostics and artifacts

The useful diagnostics remain in the common runner. **`--diagnostics` is opt-in**:
it adds `--trace-jsonl PATH` only for Crabcode and reads Claude's structured stream.
Other agents' unavailable measurements remain unknown. No prose is parsed into
invented tool counts or token usage.

Crabcode print tracing writes **only to the named JSONL file**, not stdout. Normal
stdout remains the final answer; commentary and warnings remain stderr. Without
`--trace-jsonl`, no trace file is opened or written, so there is no tracing disk-I/O
cost. Internal diagnostic events still have some processing overhead. With tracing
on, per-event serialization and file writes have a small, unquantified overhead;
trace is observational, not guaranteed zero-cost.

Available measurements include:

- Model turns: logical provider steps for Crabcode, distinct assistant message IDs
  for Claude. Split thinking/text/tool blocks are deduplicated. Neither count is an
  exact HTTP request count or visibility into internal provider work.
- Tool sequence, argument fingerprints, repeated calls, and local tool timing.
  Parallel tool timing is an interval union rather than a sum.
- Startup to first provider step, first-response timing, provider-response time,
  retries, tool errors, and per-step source/context sizes where available.
- Reported input/output/cache usage, kept separate from whole-stream metrics to
  avoid double-counting. Legacy prompt/output-size estimates remain labeled as
  estimates and are not billed usage or accurate subscription costs.
- Requested versus sent effort. A sent field does not prove the backend honors it
  or that two routes use equal thinking budgets. Signed/redacted thinking is unknown;
  zero observed reasoning bytes does not prove thinking is off.

Each attempt saves command, prompt, stdout/stderr, checks, and available trace/per-step
JSON. Clean-room fixtures also save before/after source snapshots. Trace files contain
model/tool identifiers and argument hashes, but not raw prompt/response text or tool
arguments/results. Trace destinations must be new files and are `0600` on Unix.
Raw agent logs are separate artifacts and can contain task/tool content.

Incremental JSON is always written under `.benchmarks/<run-id>/results.json`;
`--out FILE` additionally writes a chosen JSON path. Markdown defaults to
`benchmark-reports/agent-benchmark-<run-id>.md` or `--report FILE`. Artifacts remain
when workspaces are deleted; `--keep` preserves workspaces. Reports record models,
flags, task hashes, source revision, platform, and fixture configuration. Exit codes
are `0` for all passing trials, `1` for failed trials, and `130` for interruption.

Small synthetic tasks are regression probes, not representative productivity
measurements. Use repeated, held-out tasks and inspect failure artifacts before
changing the harness. Never tune a general completion policy to disclosed hidden
assertions. Backend/auth/transport differences remain explicit confounds.

## Layout and extending the suite

```text
benchmarking/
  bench-agents.ts                 Single runner
  src/
    agents.ts                    Commands and per-agent model mapping
    cli.ts                       Flags, filtering, and opt-in agents
    types.ts                     Shared tasks, attempts, and results
    report.ts                    Legacy estimates and measured verification tables
    checks.ts                    Task-specific and clean-room grading
    diagnostics.ts               Trace/stream measurements only
    workspace.ts                 Files, artifacts, and process lifecycle
    tasks/
      index.ts                   One shared task registry
      coding.ts                  Opt-in smoke fixtures
      workflow.ts                Opt-in medium workflow fixture
      basic.ts, rust.ts, …       Existing fixture modules
```

Add fixtures under `src/tasks/` and export them from `tasks/index.ts`. A task supplies
`id`, `title`, `prompt`, `files`, and a deterministic `check(cwd)`. Optional fields include
`model`, `difficulty`, `tags`, `timeoutMs`, `site`, `defaultEnabled`, and `grader`.
A clean-room task supplies its immutable fixture as `grader`; its public contract
must state all graded edge cases. Update offline baseline/reference/mutation tests
before running live agents. Use `just test-bench` to validate without model calls.
