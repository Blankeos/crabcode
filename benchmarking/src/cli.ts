import {
  AVAILABLE_AGENTS,
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

const BOOLEAN_OPTIONS = new Set(['help', 'list-tasks', 'estimate', 'no-report', 'keep', 'diagnostics', 'parallel'])
const VALUE_OPTIONS = new Set([
  'model',
  'agents',
  'tasks',
  'tags',
  'difficulty',
  'runs',
  'timeout-ms',
  'input-price',
  'output-price',
  'out',
  'report',
  'report-dir',
  'dir',
  'claude-model',
  'claude-effort',
  'claude-tools',
  'max-attempts',
  'tool-profile',
  'reasoning-effort',
])

export function parseArgs(raw: string[]): ParsedArgs {
  const parsed: ParsedArgs = {}
  for (let i = 0; i < raw.length; i++) {
    const arg = raw[i]
    if (!arg.startsWith('--')) {
      throw new Error(`Unexpected argument: ${arg}. Use --help for available options`)
    }
    const body = arg.slice(2)
    const equals = body.indexOf('=')
    const key = equals < 0 ? body : body.slice(0, equals)
    const inlineValue = equals < 0 ? undefined : body.slice(equals + 1)
    if (BOOLEAN_OPTIONS.has(key)) {
      if (inlineValue !== undefined) throw new Error(`--${key} does not take a value`)
      parsed[key] = true
      continue
    }
    if (!VALUE_OPTIONS.has(key)) {
      throw new Error(`Unknown option: --${key}. Use --help for available options`)
    }
    const value = inlineValue ?? raw[i + 1]
    if (!value?.trim() || (inlineValue === undefined && value.startsWith('-'))) {
      throw new Error(`--${key} requires a value`)
    }
    parsed[key] = value
    if (inlineValue === undefined) i++
  }
  return parsed
}

export function parseAgents(value: string): AgentName[] {
  const agents = splitCsv(value, '--agents')
  const seen = new Set<string>()
  for (const agent of agents) {
    assertAgentName(agent)
    if (seen.has(agent)) throw new Error(`Duplicate agent: ${agent}`)
    seen.add(agent)
  }
  return agents as AgentName[]
}

export function selectTasks(
  tasks: BenchmarkTask[],
  value?: string | boolean,
  tags?: string | boolean,
  difficulty?: string | boolean,
): BenchmarkTask[] {
  let selected = parseTaskIds(tasks, value)
  if (value === undefined && tags === undefined && difficulty === undefined) {
    selected = selected.filter((task) => task.defaultEnabled !== false)
  }

  if (tags !== undefined) {
    const requiredTags = splitCsv(filterValue(tags, '--tags'), '--tags')
    selected = selected.filter((task) => requiredTags.every((tag) => task.tags?.includes(tag)))
  }

  if (difficulty !== undefined) {
    const requestedDifficulty = filterValue(difficulty, '--difficulty')
    selected = selected.filter((task) => task.difficulty === requestedDifficulty)
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
    console.log(`  ${task.id.padEnd(24)} ${task.title}${tags} [${difficulty}]`)
  }
}

export function printHelp(tasks: BenchmarkTask[]) {
  const defaultTasks = tasks.filter((task) => task.defaultEnabled !== false)
  console.log(`Usage: bun run scripts/bench-agents.ts [options]

Options:
  --model provider/model             Global model for all tasks/agents, except --claude-model.
  --agents ${DEFAULT_AGENTS.join(',')}   Agents to run; add claude explicitly to opt in.
  --claude-model claude-sonnet-...     Claude-only model, independent of --model.
  --claude-effort low                 Claude-only effort (default low).
  --claude-tools Read,Write,Edit,Bash  Override Claude's tool list.
  --reasoning-effort medium           Crabcode effort (overrides BENCH_CRABCODE_REASONING).
  --tool-profile native               Tool profile: native or coding (matching coding tools).
  --tasks id-a,id-b                   Task IDs to run, including opt-in fixtures.
  --tags typescript,hidden-tests      Run tasks containing every listed tag (includes opt-ins).
  --difficulty hard                  Run tasks by difficulty: smoke, medium, hard (includes opt-ins).
  --list-tasks                       Print all available tasks, including opt-ins, and exit.
  --runs 1                           Repetitions per agent/task.
  --max-attempts 1                    Maximum attempts per run, including the first attempt.
  --diagnostics                      Capture traces and per-step diagnostics where supported.
  --parallel                         Run agents concurrently; timing comparisons are confounded.
  --timeout-ms ${DEFAULT_TIMEOUT_MS}                 Default timeout per run.
  --estimate                         Print planned prompt count and prompt-only cost, then exit.
  --input-price 1.25                 Input USD per 1M tokens for rough cost estimates.
  --output-price 10                  Output USD per 1M tokens for rough cost estimates.
  --out bench-results.json           Write machine-readable JSON results.
  --report benchmark.md              Write Markdown report at an exact path.
  --report-dir benchmark-reports     Directory for default Markdown reports.
  --no-report                        Disable Markdown report generation.
  --dir .benchmarks                  Parent directory for benchmark runs.
  --keep                             Keep temporary workspaces for inspection.
  --help                             Print this help and exit.

Available agents: ${AVAILABLE_AGENTS.join(', ')}
Claude is opt-in and runs the same selected tasks. It requires --claude-model,
BENCH_CLAUDE_MODEL, or a Claude --model; OpenAI/GPT ids are rejected, never substituted.
Claude model ids strip one provider prefix; aliases haiku, sonnet, opus, fable are supported.

Default params:
  model: per-task — ${DEFAULT_MODEL} (smoke/medium), openai/gpt-5.5 (hard); override with --model
  agents: ${DEFAULT_AGENTS.join(',')} (Claude excluded)
  tasks: ${defaultTasks.map((task) => task.id).join(',')}
  runs: ${DEFAULT_RUNS}
  max-attempts: 1
  tool-profile: native
  timeout-ms: ${DEFAULT_TIMEOUT_MS}
  input-price: ${DEFAULT_INPUT_USD_PER_MTOK}
  output-price: ${DEFAULT_OUTPUT_USD_PER_MTOK}
  dir: ${DEFAULT_BENCHMARK_DIR}
  report-dir: ${DEFAULT_REPORT_DIR}

Environment overrides:
  BENCH_MODEL, BENCH_AGENTS, BENCH_TASKS, BENCH_TAGS, BENCH_DIFFICULTY,
  BENCH_RUNS, BENCH_TIMEOUT_MS, BENCH_INPUT_USD_PER_MTOK,
  BENCH_OUTPUT_USD_PER_MTOK, BENCH_DIR, BENCH_REPORT_DIR,
  BENCH_CRABCODE_REASONING, BENCH_CLAUDE_MODEL, BENCH_CLAUDE_EFFORT

Stop behavior:
  Ctrl+C stops active agent process trees and removes temporary workspaces unless --keep is set.

Command overrides:
  BENCH_CRABCODE_CMD='crabcode -p -m {model} --reasoning-effort {effort} --no-session-persistence --dangerously-skip-permissions {prompt}'
  BENCH_OPENCODE_CMD='opencode run --dangerously-skip-permissions -m {model} {prompt}'
  BENCH_CODEX_CMD='codex exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}'
  BENCH_GROK_BUILD_CMD='grok --always-approve -m {model} -p {prompt}'
  BENCH_CLAUDE_CMD overrides the Claude command; the requested model must still be Claude-compatible.
  For crabcode diagnostics, BENCH_CRABCODE_CMD must explicitly include --trace-jsonl {trace}.
  Claude defaults isolate settings/MCP in safe mode and emit stream-json; custom commands must opt in themselves.

Binary overrides (relative paths resolve from the repository root):
  BENCH_CRABCODE_BIN, BENCH_OPENCODE_BIN, BENCH_CODEX_BIN, BENCH_GROK_BUILD_BIN, BENCH_CLAUDE_BIN

Template tokens: {prompt}, {model}, {repo}, {effort}, {trace}
  Values are shell-quoted; {trace} is empty when diagnostics are not requested.
  {model} is agent-aware: codex strips openai/; grok-build and claude strip one provider prefix.
  Same-model runs with OpenAI ids may fail on grok-build (xAI models only unless configured).

Default task list:
`)
  printTaskList(defaultTasks)
}

function parseTaskIds(tasks: BenchmarkTask[], value?: string | boolean): BenchmarkTask[] {
  if (value === undefined) return tasks
  const ids = splitCsv(filterValue(value, '--tasks'), '--tasks')
  return ids.map((id) => {
    const task = tasks.find((candidate) => candidate.id === id)
    if (!task) {
      throw new Error(`Unknown task: ${id}. Expected one of ${tasks.map((task) => task.id).join(', ')}`)
    }
    return task
  })
}

function filterValue(value: string | boolean, option: string) {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${option} requires a value`)
  return value.trim()
}

function splitCsv(value: string, option: string) {
  const items = value.split(',').map((item) => item.trim())
  if (items.some((item) => !item)) throw new Error(`${option} requires a non-empty comma-separated list`)
  return items
}
