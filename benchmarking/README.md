# Benchmarking

The agent benchmark suite compares `crabcode`, `opencode`, and `codex` on small deterministic coding tasks.

The developer UX stays anchored on the existing recipe:

```sh
just bench-agents
```

Useful filters:

```sh
just bench-agents --list-tasks
just bench-agents --tasks workflow-planner-ts
just bench-agents --tags typescript,hidden-tests
just bench-agents --difficulty hard
just bench-agents --estimate --agents crabcode,codex
```

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

