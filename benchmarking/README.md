# Benchmarking

The agent benchmark suite compares `crabcode`, `opencode`, `codex`, and `grok-build` on small deterministic coding tasks.

## Agent entry

One recipe — all control is **args / env**:

```sh
# Defaults — all tasks × all harnesses, per-task model tier, crabcode reasoning medium
just bench-agents

# Same model for every agent/task
just bench-agents --model openai/gpt-5.5
just bench-agents --model grok-4.5

# Subset of tasks
just bench-agents --list-tasks
just bench-agents --tasks bugfix-js,add-rust-test --model openai/gpt-5.5

# Subset of harnesses
just bench-agents --agents crabcode,grok-build
just bench-agents --agents crabcode,opencode --model openai/gpt-5.5

# Combine filters
just bench-agents --agents crabcode,grok-build --tasks bugfix-js --model grok-4.5

# Crabcode reasoning (env; default medium)
BENCH_CRABCODE_REASONING=high just bench-agents --model openai/gpt-5.5

# Other flags
just bench-agents --tags typescript,hidden-tests
just bench-agents --difficulty hard
just bench-agents --estimate
just bench-agents --help
```

**Reasoning:** crabcode via `BENCH_CRABCODE_REASONING` (default `medium`).
Other harnesses: `BENCH_OPENCODE_CMD` / `BENCH_CODEX_CMD` / `BENCH_GROK_BUILD_CMD` if needed.

**Models:** OpenAI ids for crabcode/opencode/codex; **grok-build** usually needs an xAI model
(or omit it from `--agents`). Binary: PATH `grok` or `BENCH_GROK_BUILD_BIN`.

## Models

OpenAI `gpt-5.3-codex` is no longer used. Default selection (unless `--model` / `BENCH_MODEL` forces one model for the whole run):

| Tier | Model |
|------|--------|
| Most tasks (smoke / medium) | `openai/gpt-5.3-codex-spark` |
| Hard tasks (`workflow-planner-ts`, `issue-triage-pipeline-ts`, or `difficulty: hard`) | `openai/gpt-5.5` |

Optional per-task `model` on a task definition overrides the tier default. Use `openai/gpt-5.4-mini` on specific tasks when spark is not appropriate.

## Layout

```text
benchmarking/
  bench-agents.ts          CLI entrypoint and run orchestration
  src/
    agents.ts             Agent command templates and prompt wrapping
    checks.ts             Reusable deterministic check helpers
    cli.ts                Args, env overrides, task filtering, help text
    defaults.ts           Default model, prices, paths, agents
    format.ts             Formatting, shell quoting, rough token/cost estimates
    report.ts             Markdown summary report writer
    static-server.ts      Per-run localhost fixture server
    workspace.ts          Fixtures, run directories, logs, cleanup
    tasks/                Benchmark task registry
```

## Adding A Benchmark

Add a task file under `benchmarking/src/tasks/` or extend an existing topic file, then export it from `benchmarking/src/tasks/index.ts`.

Tasks should be self-contained: fixture files, the exact user prompt, and deterministic checks all live with the task definition.

```ts
import { defineTask } from './define.ts'

export const myTasks = [
  defineTask({
    id: 'hard-refactor-example',
    title: 'Refactor a small module without changing behavior',
    difficulty: 'hard',
    tags: ['typescript', 'refactor'],
    files: {
      'package.json': JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n',
      'src/example.ts': `export function example() { return 1 }\n`,
    },
    prompt: `Refactor src/example.ts and keep behavior unchanged. Do not add dependencies.`,
    check: (cwd) => [
      // Prefer reusable helpers from ../checks.ts when possible.
    ],
  }),
]
```

Keep prompts direct and checks deterministic. For harder tasks, prefer visible tests plus hidden tests injected by `bunTestWithHiddenFileCheck`.
