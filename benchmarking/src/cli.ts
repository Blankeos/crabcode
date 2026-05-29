import {
  DEFAULT_AGENTS,
  DEFAULT_BENCHMARK_DIR,
  DEFAULT_INPUT_USD_PER_MTOK,
  DEFAULT_MODEL,
  DEFAULT_OUTPUT_USD_PER_MTOK,
  DEFAULT_REPORT_DIR,
  DEFAULT_RUNS,
  DEFAULT_TIMEOUT_MS,
} from './defaults.ts'
import { assertAgentName } from './agents.ts'
import type { AgentName, BenchmarkTask, ParsedArgs } from './types.ts'

export function parseArgs(raw: string[]): ParsedArgs {
  const parsed: ParsedArgs = {}
  for (let i = 0; i < raw.length; i++) {
    const arg = raw[i]
    if (!arg.startsWith('--')) continue
    const body = arg.slice(2)
    const [key, inlineValue] = body.split('=', 2)
    if (inlineValue !== undefined) {
      parsed[key] = inlineValue
      continue
    }
    const next = raw[i + 1]
    if (next && !next.startsWith('--')) {
      parsed[key] = next
      i++
    } else {
      parsed[key] = true
    }
  }
  return parsed
}

export function parseAgents(value: string): AgentName[] {
  const agents = value
    .split(',')
    .map((agent) => agent.trim())
    .filter(Boolean)

  for (const agent of agents) {
    assertAgentName(agent)
  }

  return agents as AgentName[]
}

export function selectTasks(tasks: BenchmarkTask[], value?: string | boolean, tags?: string | boolean, difficulty?: string | boolean): BenchmarkTask[] {
  let selected = parseTaskIds(tasks, value)

  if (tags && tags !== true) {
    const requiredTags = splitCsv(String(tags))
    selected = selected.filter((task) => requiredTags.every((tag) => task.tags?.includes(tag)))
  }

  if (difficulty && difficulty !== true) {
    selected = selected.filter((task) => task.difficulty === difficulty)
  }

  if (!selected.length) {
    throw new Error('No benchmark tasks matched the requested filters')
  }

  return selected
}

export function printTaskList(tasks: BenchmarkTask[]) {
  console.log('Benchmark tasks')
  for (const task of tasks) {
    const difficulty = task.difficulty ?? 'medium'
    const tags = task.tags?.length ? ` [${task.tags.join(', ')}]` : ''
    console.log(`  ${task.id.padEnd(24)} ${difficulty.padEnd(6)} ${task.title}${tags}`)
  }
}

export function printHelp(tasks: BenchmarkTask[]) {
  console.log(`Usage: bun run scripts/bench-agents.ts [options]

Options:
  --model provider/model             Model passed to each agent.
  --agents crabcode,opencode,codex   Agents to run.
  --tasks id-a,id-b                  Task IDs to run.
  --tags typescript,hidden-tests     Run tasks containing every listed tag.
  --difficulty hard                  Run tasks by difficulty: smoke, medium, hard.
  --list-tasks                       Print available tasks and exit.
  --runs 1                           Repetitions per agent/task.
  --timeout-ms 45000                 Timeout per run.
  --estimate                         Print planned prompt count and prompt-only cost, then exit.
  --input-price 1.25                 Input USD per 1M tokens for rough cost estimates.
  --output-price 10                  Output USD per 1M tokens for rough cost estimates.
  --out bench-results.json           Write machine-readable JSON results.
  --report benchmark.md              Write Markdown report at an exact path.
  --report-dir benchmark-reports     Directory for default Markdown reports.
  --no-report                        Disable Markdown report generation.
  --dir .benchmarks                  Parent directory for benchmark runs.
  --keep                             Keep temporary workspaces for inspection.

Default params:
  model: ${DEFAULT_MODEL}
  agents: ${DEFAULT_AGENTS.join(',')}
  tasks: ${tasks.map((task) => task.id).join(',')}
  runs: ${DEFAULT_RUNS}
  timeout-ms: ${DEFAULT_TIMEOUT_MS}
  input-price: ${DEFAULT_INPUT_USD_PER_MTOK}
  output-price: ${DEFAULT_OUTPUT_USD_PER_MTOK}
  dir: ${DEFAULT_BENCHMARK_DIR}
  report-dir: ${DEFAULT_REPORT_DIR}

Environment overrides:
  BENCH_MODEL, BENCH_AGENTS, BENCH_TASKS, BENCH_TAGS, BENCH_DIFFICULTY,
  BENCH_RUNS, BENCH_TIMEOUT_MS, BENCH_INPUT_USD_PER_MTOK,
  BENCH_OUTPUT_USD_PER_MTOK, BENCH_DIR, BENCH_REPORT_DIR

Stop behavior:
  Ctrl+C stops the active agent process tree and removes temporary workspaces unless --keep is set.

Command overrides:
  BENCH_CRABCODE_CMD='crabcode -p --no-session-persistence --dangerously-skip-permissions {prompt}'
  BENCH_OPENCODE_CMD='opencode run --dangerously-skip-permissions -m {model} {prompt}'
  BENCH_CODEX_CMD='codex exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}'

Template tokens: {prompt}, {model}, {repo}
Note: {model} is agent-aware; codex strips a leading openai/ provider prefix.
`)
}

function parseTaskIds(tasks: BenchmarkTask[], value?: string | boolean): BenchmarkTask[] {
  if (!value || value === true) return tasks
  const ids = splitCsv(String(value))
  return ids.map((id) => {
    const task = tasks.find((candidate) => candidate.id === id)
    if (!task) {
      throw new Error(`Unknown task: ${id}. Expected one of ${tasks.map((task) => task.id).join(', ')}`)
    }
    return task
  })
}

function splitCsv(value: string) {
  return value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
}
