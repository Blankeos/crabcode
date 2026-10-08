// One runner for every agent and task. Claude is available, never selected by default.
import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import {
  benchmarkPrompt,
  commandFor,
  crabcodeFixtureConfig,
  displayAgent,
  modelForAgent,
  resolveTaskPrompt,
  repairPrompt,
  RULES,
  type AgentCommandOptions,
} from './src/agents.ts'
import { parseAgents, parseArgs, printHelp, printTaskList, selectTasks } from './src/cli.ts'
import { gradeTask, runChecks } from './src/checks.ts'
import {
  DEFAULT_AGENTS,
  DEFAULT_INPUT_USD_PER_MTOK,
  DEFAULT_OUTPUT_USD_PER_MTOK,
  DEFAULT_REPORT_DIR,
  DEFAULT_RUNS,
  DEFAULT_TIMEOUT_MS,
  REPO_ROOT,
} from './src/defaults.ts'
import { estimateCost, estimateTokens, formatDuration, pairedOrder, tailText } from './src/format.ts'
import { telemetry } from './src/diagnostics.ts'
import { resolveBenchmarkModel } from './src/models.ts'
import { pairedSummary, summaryRows, writeMarkdownReport } from './src/report.ts'
import { startStaticServer } from './src/static-server.ts'
import { TASKS } from './src/tasks/index.ts'
import {
  cleanupWorkspaceChildren,
  createRunRoot,
  fixtureHash,
  runProcess,
  sourceDiff,
  timestampForPath,
  writeFiles,
} from './src/workspace.ts'
import type {
  AgentName,
  BenchmarkAttempt,
  BenchmarkTask,
  CheckResult,
  Grade,
  RunResult,
  ToolProfile,
} from './src/types.ts'

const args = parseArgs(process.argv.slice(2))
if (args.help) {
  printHelp(TASKS)
  process.exit(0)
}
if (args['list-tasks']) {
  printTaskList(TASKS)
  process.exit(0)
}
const agents = parseAgents(String(args.agents ?? process.env.BENCH_AGENTS ?? DEFAULT_AGENTS.join(',')))
const tasks = selectTasks(
  TASKS,
  args.tasks ?? process.env.BENCH_TASKS,
  args.tags ?? process.env.BENCH_TAGS,
  args.difficulty ?? process.env.BENCH_DIFFICULTY,
)
const modelOverride = args.model || process.env.BENCH_MODEL ? String(args.model ?? process.env.BENCH_MODEL) : undefined
function integer(value: unknown, name: string) {
  const result = Number(value)
  if (!Number.isSafeInteger(result) || result < 1) throw new Error(`${name} must be a positive integer`)
  return result
}
const runs = integer(args.runs ?? process.env.BENCH_RUNS ?? DEFAULT_RUNS, '--runs')
const timeoutMs = integer(args['timeout-ms'] ?? process.env.BENCH_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS, '--timeout-ms')
const maxAttempts = integer(args['max-attempts'] ?? 1, '--max-attempts')
const diagnostics = Boolean(args.diagnostics)
const parallel = Boolean(args.parallel)
const keep = Boolean(args.keep)
const toolProfile = String(args['tool-profile'] ?? 'native') as ToolProfile
if (!['coding', 'native'].includes(toolProfile)) throw new Error('--tool-profile must be coding or native')
const claudeModel =
  args['claude-model'] || process.env.BENCH_CLAUDE_MODEL
    ? String(args['claude-model'] ?? process.env.BENCH_CLAUDE_MODEL)
    : undefined
const claudeEffort = String(args['claude-effort'] ?? process.env.BENCH_CLAUDE_EFFORT ?? 'low')
if (!['low', 'medium', 'high', 'xhigh', 'max'].includes(claudeEffort)) throw new Error('Invalid --claude-effort')
const claudeTools = args['claude-tools'] ? String(args['claude-tools']) : undefined
const reasoningEffort = String(args['reasoning-effort'] ?? process.env.BENCH_CRABCODE_REASONING ?? 'medium')
if (!['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(reasoningEffort)) {
  throw new Error('Invalid --reasoning-effort')
}
const inputPrice = Number(args['input-price'] ?? process.env.BENCH_INPUT_USD_PER_MTOK ?? DEFAULT_INPUT_USD_PER_MTOK)
const outputPrice = Number(args['output-price'] ?? process.env.BENCH_OUTPUT_USD_PER_MTOK ?? DEFAULT_OUTPUT_USD_PER_MTOK)
if (![inputPrice, outputPrice].every((value) => Number.isFinite(value) && value >= 0))
  throw new Error('Invalid token price')
const commandOptions: AgentCommandOptions = { claudeModel, claudeEffort, claudeTools, reasoningEffort, toolProfile }
const taskModel = (task: BenchmarkTask) => resolveBenchmarkModel(task, modelOverride)
const agentModel = (agent: AgentName, task: BenchmarkTask) =>
  modelForAgent(agent, agent === 'claude' && claudeModel ? claudeModel : taskModel(task))
// Validate every selected combination before creating workspaces or invoking a model.
for (const task of tasks) {
  if (task.timeoutMs !== undefined) integer(task.timeoutMs, `${task.id} timeout`)
  for (const agent of agents)
    commandFor(agent, 'Preflight only', taskModel(task), {
      ...commandOptions,
      ...(diagnostics && agent === 'crabcode' ? { tracePath: resolve('trace-preflight.jsonl') } : {}),
    })
}
const plannedPrompts = tasks.length * agents.length * runs
const estimatedInputTokens = tasks.reduce(
  (sum, task) =>
    sum +
    estimateTokens(task.grader ? repairPrompt(task.grader, []) : benchmarkPrompt(resolveTaskPrompt(task))) *
      agents.length *
      runs,
  0,
)
console.log(
  `Agent benchmark: ${agents.map(displayAgent).join(', ')}\n${tasks.length} tasks × ${runs} repetitions; up to ${maxAttempts} attempt(s) per trial.`,
)
console.log(
  `Tool profile: ${toolProfile}; mode: ${parallel ? 'parallel (shared-load timing)' : 'alternating sequential'}.`,
)
if (args.estimate) {
  console.log(
    `Planned initial prompts: ${plannedPrompts}; prompt-only estimate: ${estimatedInputTokens} tokens, $${estimateCost(estimatedInputTokens, 0, inputPrice, outputPrice).toFixed(4)}. Excludes history/tools/repairs.`,
  )
  process.exit(0)
}

const runId = timestampForPath()
const runRoot = createRunRoot(args.dir ?? process.env.BENCH_DIR, runId)
const workspacesRoot = join(runRoot, 'workspaces')
const logsRoot = join(runRoot, 'logs')
const configHome = join(runRoot, 'config')
for (const path of [workspacesRoot, logsRoot, configHome]) mkdirSync(path, { recursive: true })
const outputPath = args.out ? resolve(String(args.out)) : null
const reportPath = args['no-report']
  ? null
  : args.report
    ? resolve(String(args.report))
    : join(
        resolve(String(args['report-dir'] ?? process.env.BENCH_REPORT_DIR ?? DEFAULT_REPORT_DIR)),
        `agent-benchmark-${runId}.md`,
      )
const controller = new AbortController()
let stopped = false
let runnerError: string | null = null
function stop() {
  stopped = true
  controller.abort()
}
process.once('SIGINT', stop)
process.once('SIGTERM', stop)
const results: RunResult[] = []
const metadata = {
  schemaVersion: 2,
  generatedAt: new Date().toISOString(),
  runId,
  runRoot,
  workspacesRoot,
  logsRoot,
  markdownReport: reportPath,
  model: modelOverride ?? null,
  modelSelection: Object.fromEntries(tasks.map((task) => [task.id, taskModel(task)])),
  agentModels: Object.fromEntries(
    agents.map((agent) => [agent, Object.fromEntries(tasks.map((task) => [task.id, agentModel(agent, task)]))]),
  ),
  agents,
  tasks: tasks.map((task) => task.id),
  runs,
  maxAttempts,
  timeoutMs,
  toolProfile,
  diagnostics,
  parallel,
  claude: agents.includes('claude')
    ? { model: claudeModel ?? null, effort: claudeEffort, tools: claudeTools ?? null }
    : null,
  reasoningEffort,
  platform: `${process.platform}/${process.arch}`,
  bun: Bun.version,
  revision: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: REPO_ROOT, encoding: 'utf8' }).stdout?.trim(),
  dirty: Boolean(spawnSync('git', ['status', '--porcelain'], { cwd: REPO_ROOT, encoding: 'utf8' }).stdout?.trim()),
  fixtures: tasks.map((task) => ({
    id: task.id,
    difficulty: task.difficulty,
    sha256: fixtureHash(
      task.grader ?? { id: task.id, title: task.title, prompt: task.prompt, files: task.files, checks: {} },
    ),
    grading: task.grader ? 'immutable clean-room source grader' : 'task-specific checks',
  })),
  policy:
    'Fresh CLI conversation per attempt; same edited workspace; original prompt plus cumulative verifier feedback on repair.',
  pricing: { inputUsdPerMillionTokens: inputPrice, outputUsdPerMillionTokens: outputPrice },
  caveats: [
    'Requested model/effort does not prove matched upstream backends or thinking budgets.',
    'First attempt includes native tool/test iterations; repaired success is verifier-assisted, not autonomous pass@k.',
    'Coding profile matches six capabilities only for Crabcode/Claude; other agents retain native tools. Custom Claude tools are explicitly unmatched.',
    'Measured telemetry is separate from legacy prompt/output-size token estimates. Missing telemetry is null, not inferred from prose.',
    'Disposable workspaces are not a security sandbox; candidate source and tools execute locally.',
  ],
}
function save() {
  const report = { ...metadata, stopped, runnerError, paired: pairedSummary(results), results }
  for (const path of [join(runRoot, 'results.json'), ...(outputPath ? [outputPath] : [])]) {
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, JSON.stringify(report, null, 2) + '\n')
  }
  if (reportPath)
    writeMarkdownReport(reportPath, {
      runId,
      runRoot,
      workspacesRoot,
      logsRoot,
      model: modelOverride ?? '(per-task)',
      modelSelection: metadata.modelSelection,
      agents,
      tasks,
      runs,
      plannedPrompts,
      timeoutMs,
      keep,
      inputPrice,
      outputPrice,
      results,
      stopped,
      maxAttempts,
      toolProfile,
      diagnostics,
      claudeModel,
      claudeEffort: agents.includes('claude') ? claudeEffort : undefined,
      claudeTools,
      reasoningEffort,
      parallel,
    })
}
console.log(
  `Workspaces: ${workspacesRoot}\nArtifacts: ${logsRoot}\nJSON: ${outputPath ?? join(runRoot, 'results.json')}`,
)
if (reportPath) console.log(`Report: ${reportPath}`)
save()

function asGrade(checks: CheckResult[]): Grade {
  return {
    passed: checks.length > 0 && checks.every((check) => check.pass),
    checks: checks.map((check) => ({ name: check.name, passed: check.pass, output: check.detail ?? '' })),
  }
}
function gradedChecks(task: BenchmarkTask, workspace: string): CheckResult[] {
  return task.grader
    ? gradeTask(task.grader, workspace).checks.map((check) => ({
        name: check.name,
        pass: check.passed,
        detail: check.output,
      }))
    : runChecks(task, workspace)
}
function followUpPrompt(task: BenchmarkTask, initial: string, attempts: BenchmarkAttempt[]) {
  if (task.grader)
    return repairPrompt(
      task.grader,
      attempts.map((attempt) => asGrade(attempt.checks)),
    )
  const feedback = attempts.map(
    (attempt) =>
      `Verification after attempt ${attempt.number}:\n${attempt.checks
        .filter((check) => !check.pass)
        .map((check) => `${check.name}: ${check.detail ?? 'failed'}`)
        .join('\n')
        .slice(-12_000)}`,
  )
  return [
    initial,
    ...feedback,
    attempts.length ? 'Continue fixing current files. This is a fresh conversation; previous edits remain.' : '',
  ]
    .filter(Boolean)
    .join('\n\n')
}

async function runTrial(agent: AgentName, task: BenchmarkTask, repetition: number): Promise<RunResult> {
  const label = `${task.id}-${agent}-${repetition}`
  const workspace = join(workspacesRoot, label)
  const artifactsRoot = join(logsRoot, label)
  const model = agentModel(agent, task)
  const attempts: BenchmarkAttempt[] = []
  const budget = task.timeoutMs ?? timeoutMs
  let server: Awaited<ReturnType<typeof startStaticServer>> | undefined
  let elapsedAgentMs = 0,
    elapsedMs = 0,
    estimatedInputTokens = 0,
    estimatedOutputTokens = 0
  let ok = false,
    error: string | undefined,
    stdoutTail = '',
    stderrTail = ''
  console.log(`→ ${label} (${model})`)
  try {
    mkdirSync(workspace)
    mkdirSync(artifactsRoot)
    writeFiles(workspace, task.files)
    const isolate = agents.includes('claude') || Boolean(task.grader) || diagnostics || toolProfile === 'coding'
    const config = isolate ? crabcodeFixtureConfig(taskModel(task), toolProfile) : { model: taskModel(task) }
    writeFiles(workspace, { 'crabcode.jsonc': JSON.stringify(config, null, 2) + '\n' })
    writeFileSync(join(artifactsRoot, 'workspace-config.json'), JSON.stringify(config, null, 2) + '\n')
    if (isolate) {
      writeFiles(workspace, { 'AGENTS.md': RULES + '\n', 'CLAUDE.md': RULES + '\n' })
      const git = spawnSync('git', ['init', '-q'], { cwd: workspace, encoding: 'utf8' })
      if (git.status !== 0) throw new Error(`Cannot initialize fixture: ${git.stderr}`)
    }
    if (task.grader && asGrade(gradedChecks(task, workspace)).passed)
      throw new Error('Broken baseline unexpectedly passes')
    if (task.site) server = await startStaticServer(join(workspace, task.site.root))
    const initial = task.grader ? repairPrompt(task.grader, []) : benchmarkPrompt(resolveTaskPrompt(task, server?.url))
    const started = performance.now()
    for (let number = 1; number <= maxAttempts && !controller.signal.aborted; number++) {
      const remaining = budget - elapsedAgentMs
      if (remaining <= 0) break
      const prompt = followUpPrompt(task, initial, attempts)
      const path = join(artifactsRoot, `attempt-${number}`)
      mkdirSync(path)
      const tracePath = diagnostics && agent === 'crabcode' ? join(path, 'trace.jsonl') : undefined
      const command = commandFor(agent, prompt, taskModel(task), { ...commandOptions, tracePath })
      const artifacts = {
        commandPath: join(path, 'command.txt'),
        stdoutPath: join(path, 'stdout.log'),
        stderrPath: join(path, 'stderr.log'),
        promptPath: join(path, 'prompt.txt'),
        tracePath,
        stepsPath: diagnostics ? join(path, 'steps.json') : undefined,
        sourceDiffPath: task.grader ? join(path, 'source-diff.json') : undefined,
      }
      writeFileSync(artifacts.commandPath, command + '\n')
      writeFileSync(artifacts.promptPath, prompt)
      const proc = await runProcess(
        {
          bin: process.platform === 'win32' ? 'cmd.exe' : '/bin/sh',
          args: process.platform === 'win32' ? ['/c', command] : ['-c', command],
        },
        workspace,
        remaining,
        isolate
          ? {
              XDG_CONFIG_HOME: configHome,
              CLAUDE_CODE_DISABLE_AUTO_MEMORY: '1',
              CRABCODE_DISABLE_CLAUDE_CODE_PROMPT: '1',
            }
          : {},
        controller.signal,
      )
      elapsedAgentMs += proc.elapsedMs
      writeFileSync(artifacts.stdoutPath, proc.stdout)
      writeFileSync(artifacts.stderrPath, proc.stderr)
      const measured =
        agent === 'claude' || agent === 'crabcode'
          ? telemetry(
              agent,
              proc.stdout,
              proc.stderr,
              tracePath && existsSync(tracePath) ? readFileSync(tracePath, 'utf8') : undefined,
            )
          : undefined
      if (tracePath && !measured?.breakdown && !proc.timedOut && !proc.interrupted) {
        proc.error ??=
          'Diagnostic trace missing or invalid; verify selected Crabcode supports --trace-jsonl and custom command includes {trace}'
      }
      const checks = gradedChecks(task, workspace)
      writeFileSync(join(path, 'checks.json'), JSON.stringify(checks, null, 2) + '\n')
      if (artifacts.stepsPath)
        writeFileSync(artifacts.stepsPath, JSON.stringify(measured?.breakdown?.steps ?? null, null, 2) + '\n')
      if (artifacts.sourceDiffPath && task.grader)
        writeFileSync(artifacts.sourceDiffPath, JSON.stringify(sourceDiff(task.grader, workspace), null, 2) + '\n')
      const { stdout, stderr, ...processResult } = proc
      attempts.push({ number, process: processResult, checks, telemetry: measured, artifacts })
      stdoutTail = tailText(stdout)
      stderrTail = tailText(stderr)
      estimatedInputTokens += estimateTokens(prompt)
      estimatedOutputTokens += estimateTokens(`${stdout}\n${stderr}`)
      ok =
        !stopped &&
        !proc.error &&
        !proc.interrupted &&
        !proc.timedOut &&
        proc.exitCode === 0 &&
        !measured?.apiError &&
        asGrade(checks).passed
      error =
        [
          proc.error,
          proc.timedOut ? `timed out after ${budget}ms of agent time` : undefined,
          proc.interrupted ? 'interrupted' : undefined,
          proc.exitCode !== 0 ? `exit code ${proc.exitCode}` : undefined,
          measured?.apiError ? 'reported API error' : undefined,
          ...checks
            .filter((check) => !check.pass)
            .map((check) => `${check.name}: ${tailText(check.detail ?? 'failed', 800)}`),
        ]
          .filter(Boolean)
          .join('; ') || undefined
      console.log(
        `  ${ok ? '✓' : '✗'} attempt ${number}: ${checks.filter((check) => check.pass).length}/${checks.length} checks; ${formatDuration(proc.elapsedMs)}`,
      )
      if (ok || proc.error || proc.interrupted || proc.timedOut || proc.exitCode !== 0 || measured?.apiError) break
    }
    elapsedMs = Math.round(performance.now() - started)
  } catch (err) {
    error = err instanceof Error ? err.message : String(err)
    console.error(`  ✗ ${label}: ${error}`)
  } finally {
    await server?.close()
  }
  const last = attempts.at(-1)
  const passedChecks = last?.checks.filter((check) => check.pass).length ?? 0
  return {
    agent,
    task: task.id,
    model,
    repetition,
    ok,
    firstAttemptOk: ok && attempts.length === 1,
    verifiedAtMs: ok ? elapsedMs : null,
    attempts,
    telemetry: last?.telemetry,
    passedChecks,
    totalChecks: last?.checks.length ?? 1,
    elapsedMs,
    estimatedInputTokens,
    estimatedOutputTokens,
    estimatedCostUsd: estimateCost(estimatedInputTokens, estimatedOutputTokens, inputPrice, outputPrice),
    exitCode: last?.process.exitCode ?? null,
    timedOut: last?.process.timedOut ?? false,
    error,
    workspace,
    stdoutPath: last?.artifacts.stdoutPath,
    stderrPath: last?.artifacts.stderrPath,
    commandPath: last?.artifacts.commandPath,
    stdoutTail,
    stderrTail,
  }
}

try {
  for (let repetition = 1; repetition <= runs && !controller.signal.aborted; repetition++) {
    for (const [index, task] of tasks.entries()) {
      if (controller.signal.aborted) break
      const order = pairedOrder(index + repetition - 1, agents)
      const run = async (agent: AgentName) => {
        const result = await runTrial(agent, task, repetition)
        results.push(result)
        save()
      }
      if (parallel) {
        const pairs = order.map((agent) =>
          run(agent).catch((error) => {
            controller.abort()
            throw error
          }),
        )
        const rows = await Promise.allSettled(pairs)
        const failure = rows.find((row) => row.status === 'rejected')
        if (failure?.status === 'rejected') throw failure.reason
      } else
        for (const agent of order) {
          if (!controller.signal.aborted) await run(agent)
        }
    }
  }
} catch (error) {
  runnerError = String(error)
  throw error
} finally {
  save()
  if (!keep) cleanupWorkspaceChildren(workspacesRoot)
  process.removeListener('SIGINT', stop)
  process.removeListener('SIGTERM', stop)
}
console.table(summaryRows(results, agents))
console.log(`Saved JSON: ${outputPath ?? join(runRoot, 'results.json')}`)
if (reportPath) console.log(`Saved report: ${reportPath}`)
process.exitCode = stopped ? 130 : results.some((result) => !result.ok) ? 1 : 0
