// Make-shift benchmark for comparing crabcode, opencode, and codex on tiny agent tasks.
// Run via: `just bench-agents`

// @ts-nocheck

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { benchmarkPrompt, commandFor, displayAgent, modelForAgent, resolveTaskPrompt } from './src/agents.ts'
import { parseAgents, parseArgs, printHelp, printTaskList, selectTasks } from './src/cli.ts'
import {
  DEFAULT_AGENTS,
  DEFAULT_INPUT_USD_PER_MTOK,
  DEFAULT_MODEL,
  DEFAULT_OUTPUT_USD_PER_MTOK,
  DEFAULT_REPORT_DIR,
  DEFAULT_RUNS,
  DEFAULT_TIMEOUT_MS,
} from './src/defaults.ts'
import { estimateCost, estimateTokens, formatDuration, formatUsd, tailText } from './src/format.ts'
import { writeMarkdownReport, summaryRows } from './src/report.ts'
import { runChecks } from './src/checks.ts'
import { startStaticServer } from './src/static-server.ts'
import { TASKS } from './src/tasks/index.ts'
import {
  cleanupWorkspace,
  cleanupWorkspaceChildren,
  createRunRoot,
  timestampForPath,
  writeFixture,
  writeRunArtifacts,
} from './src/workspace.ts'
import type { AgentName, BenchmarkTask, RunResult } from './src/types.ts'

const activeChildren = new Set<any>()
const activeWorkspaces = new Set<string>()
let activeRunRoot: string | null = null
let shutdownRequested = false

process.once('SIGINT', () => requestShutdown('SIGINT'))
process.once('SIGTERM', () => requestShutdown('SIGTERM'))

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
const selectedTasks = selectTasks(
  TASKS,
  args.tasks ?? process.env.BENCH_TASKS,
  args.tags ?? process.env.BENCH_TAGS,
  args.difficulty ?? process.env.BENCH_DIFFICULTY,
)
const model = String(args.model ?? process.env.BENCH_MODEL ?? DEFAULT_MODEL)
const timeoutMs = Number(args['timeout-ms'] ?? process.env.BENCH_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
const runs = Number(args.runs ?? process.env.BENCH_RUNS ?? DEFAULT_RUNS)
const keep = Boolean(args.keep)
const estimateOnly = Boolean(args.estimate)
const inputPrice = Number(args['input-price'] ?? process.env.BENCH_INPUT_USD_PER_MTOK ?? DEFAULT_INPUT_USD_PER_MTOK)
const outputPrice = Number(args['output-price'] ?? process.env.BENCH_OUTPUT_USD_PER_MTOK ?? DEFAULT_OUTPUT_USD_PER_MTOK)
const outputPath = args.out ? resolve(String(args.out)) : null
const runId = timestampForPath()
const runRoot = createRunRoot(args.dir ?? process.env.BENCH_DIR, runId)
const workspacesRoot = join(runRoot, 'workspaces')
const logsRoot = join(runRoot, 'logs')
mkdirSync(workspacesRoot, { recursive: true })
mkdirSync(logsRoot, { recursive: true })
const reportPath = args['no-report']
  ? null
  : args.report && args.report !== true
    ? resolve(String(args.report))
    : join(resolve(String(args['report-dir'] ?? process.env.BENCH_REPORT_DIR ?? DEFAULT_REPORT_DIR)), `agent-benchmark-${runId}.md`)

if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
  throw new Error('--timeout-ms must be a positive number')
}

if (!Number.isFinite(runs) || runs <= 0) {
  throw new Error('--runs must be a positive number')
}

const plannedPrompts = selectedTasks.length * agents.length * runs
const estimatedInputTokens = selectedTasks.reduce((sum, task) => sum + estimateTokens(benchmarkPrompt(task.prompt)), 0) * agents.length * runs
const plannedCost = estimateCost(estimatedInputTokens, 0, inputPrice, outputPrice)
const maxRunTimeoutMs = Math.max(timeoutMs, ...selectedTasks.map((task) => Number(task.timeoutMs ?? timeoutMs)))

printIntro()

if (estimateOnly) {
  process.exit(0)
}

activeRunRoot = runRoot
printPaths()

const results: RunResult[] = []
writeCurrentMarkdownReport()

try {
  let runNumber = 0

  runLoop: for (let runIndex = 0; runIndex < runs; runIndex++) {
    for (const task of selectedTasks) {
      for (const agent of agents) {
        if (shutdownRequested) break runLoop
        runNumber += 1
        const result = await safeRunBenchmark(agent, task, runIndex, runNumber, plannedPrompts)
        results.push(result)
        writeCurrentMarkdownReport()
        printResult(result)
      }
    }
  }

  printSummary(results)

  if (reportPath) {
    writeCurrentMarkdownReport()
    console.log(`\nWrote Markdown report to ${reportPath}`)
  }

  if (shutdownRequested) {
    process.exitCode = 130
  }

  if (outputPath) {
    writeFileSync(
      outputPath,
      JSON.stringify(
        {
          generatedAt: new Date().toISOString(),
          model,
          agents,
          tasks: selectedTasks.map((task) => task.id),
          runs,
          runId,
          runRoot,
          workspacesRoot,
          logsRoot,
          markdownReport: reportPath,
          agentModels: Object.fromEntries(agents.map((agent) => [agent, modelForAgent(agent, model)])),
          pricing: {
            inputUsdPerMillionTokens: inputPrice,
            outputUsdPerMillionTokens: outputPrice,
          },
          results,
        },
        null,
        2,
      ) + '\n',
    )
    console.log(`\nWrote JSON results to ${outputPath}`)
  }
} finally {
  writeCurrentMarkdownReport()
  if (keep) {
    console.log(`\nKept benchmark workspaces in ${workspacesRoot}`)
  } else {
    cleanupWorkspaceChildren(workspacesRoot)
  }
  activeRunRoot = null
}

function writeCurrentMarkdownReport() {
  if (!reportPath || estimateOnly) return

  writeMarkdownReport(reportPath, {
    runId,
    runRoot,
    workspacesRoot,
    logsRoot,
    model,
    agents,
    tasks: selectedTasks,
    runs,
    plannedPrompts,
    timeoutMs,
    keep,
    inputPrice,
    outputPrice,
    results,
    stopped: shutdownRequested,
  })
}

async function safeRunBenchmark(
  agent: AgentName,
  task: BenchmarkTask,
  runIndex: number,
  runNumber: number,
  totalRuns: number,
): Promise<RunResult> {
  try {
    return await runBenchmark(agent, task, runIndex, runNumber, totalRuns)
  } catch (err) {
    return {
      agent,
      task: task.id,
      ok: false,
      passedChecks: 0,
      totalChecks: 0,
      elapsedMs: 0,
      estimatedInputTokens: 0,
      estimatedOutputTokens: 0,
      estimatedCostUsd: 0,
      exitCode: null,
      timedOut: false,
      error: `benchmark runner crashed: ${err instanceof Error ? err.message : String(err)}`,
    }
  }
}

async function runBenchmark(
  agent: AgentName,
  task: BenchmarkTask,
  runIndex: number,
  runNumber: number,
  totalRuns: number,
): Promise<RunResult> {
  const runLabel = `${String(runIndex + 1).padStart(2, '0')}-${agent}-${task.id}`
  const workspace = join(workspacesRoot, runLabel)
  const runTimeoutMs = Number(task.timeoutMs ?? timeoutMs)
  mkdirSync(workspace, { recursive: true })
  activeWorkspaces.add(workspace)
  let staticServer: Awaited<ReturnType<typeof startStaticServer>> | null = null

  try {
    writeFixture(workspace, task)

    if (model) {
      writeFileSync(join(workspace, 'crabcode.jsonc'), JSON.stringify({ model }, null, 2) + '\n')
    }

    printRunStart(runNumber, totalRuns, agent, task.id, workspace)

    if (task.site) {
      try {
        staticServer = await startStaticServer(join(workspace, task.site.root))
      } catch (err) {
        const checks = runChecks(task, workspace)
        const passedChecks = checks.filter((check) => check.pass).length
        return {
          agent,
          task: task.id,
          ok: false,
          passedChecks,
          totalChecks: checks.length,
          elapsedMs: 0,
          estimatedInputTokens: 0,
          estimatedOutputTokens: 0,
          estimatedCostUsd: 0,
          exitCode: null,
          timedOut: false,
          error: `failed to start local static server: ${err instanceof Error ? err.message : String(err)}`,
          workspace,
        }
      }
    }

    const prompt = benchmarkPrompt(resolveTaskPrompt(task, staticServer?.url))
    const command = commandFor(agent, prompt, model)
    const started = performance.now()
    const proc = await runShell(command, workspace, runTimeoutMs)
    const elapsedMs = Math.round(performance.now() - started)
    const checks = runChecks(task, workspace)
    const passedChecks = checks.filter((check) => check.pass).length
    const output = `${proc.stdout}\n${proc.stderr}`.trim()
    const artifacts = writeRunArtifacts(logsRoot, runLabel, command, proc.stdout, proc.stderr)
    const estimatedInputTokens = estimateTokens(prompt)
    const estimatedOutputTokens = estimateTokens(output)
    const ok = !shutdownRequested && !proc.timedOut && proc.exitCode === 0 && passedChecks === checks.length
    const errors = [
      proc.timedOut ? `timed out after ${runTimeoutMs}ms` : '',
      proc.exitCode !== 0 && proc.exitCode !== null ? `exit code ${proc.exitCode}` : '',
      ...checks
        .filter((check) => !check.pass)
        .map((check) => `${check.name}${check.detail ? `: ${check.detail}` : ''}`),
      proc.error ?? '',
    ].filter(Boolean)

    return {
      agent,
      task: task.id,
      ok,
      passedChecks,
      totalChecks: checks.length,
      elapsedMs,
      estimatedInputTokens,
      estimatedOutputTokens,
      estimatedCostUsd: estimateCost(estimatedInputTokens, estimatedOutputTokens, inputPrice, outputPrice),
      exitCode: proc.exitCode,
      timedOut: proc.timedOut,
      error: errors.join('; ') || undefined,
      workspace,
      stdoutPath: artifacts.stdoutPath,
      stderrPath: artifacts.stderrPath,
      commandPath: artifacts.commandPath,
      stdoutTail: tailText(proc.stdout),
      stderrTail: tailText(proc.stderr),
    }
  } finally {
    await staticServer?.close()
    activeWorkspaces.delete(workspace)
  }
}

function runShell(command: string, cwd: string, timeoutMs: number) {
  return new Promise<{ stdout: string; stderr: string; exitCode: number | null; timedOut: boolean; error?: string }>(
    (resolveRun) => {
      const child = spawn(command, {
        cwd,
        shell: true,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: process.platform !== 'win32',
        env: {
          ...process.env,
          NO_COLOR: '1',
          CI: '1',
        },
      })

      let stdout = ''
      let stderr = ''
      let timedOut = false
      let settled = false
      activeChildren.add(child)

      const timer = setTimeout(() => {
        timedOut = true
        terminateChild(child, 'SIGTERM')
        setTimeout(() => terminateChild(child, 'SIGKILL'), 2_000).unref()
      }, timeoutMs)

      child.stdout.on('data', (chunk) => {
        stdout += chunk.toString()
      })
      child.stderr.on('data', (chunk) => {
        stderr += chunk.toString()
      })
      child.on('error', (err) => {
        if (settled) return
        settled = true
        activeChildren.delete(child)
        clearTimeout(timer)
        resolveRun({ stdout, stderr, exitCode: null, timedOut, error: err.message })
      })
      child.on('close', (code) => {
        if (settled) return
        settled = true
        activeChildren.delete(child)
        clearTimeout(timer)
        resolveRun({ stdout, stderr, exitCode: code, timedOut })
      })
    },
  )
}

function requestShutdown(signal: string) {
  if (shutdownRequested) {
    writeCurrentMarkdownReport()
    cleanupActiveWorkspaces()
    process.exit(signal === 'SIGINT' ? 130 : 143)
  }

  shutdownRequested = true
  console.error(`\nReceived ${signal}; stopping active agent processes...`)
  writeCurrentMarkdownReport()

  for (const child of activeChildren) {
    terminateChild(child, 'SIGTERM')
  }

  setTimeout(() => {
    for (const child of activeChildren) {
      terminateChild(child, 'SIGKILL')
    }
    writeCurrentMarkdownReport()
    cleanupActiveWorkspaces()
    process.exit(signal === 'SIGINT' ? 130 : 143)
  }, 2_500).unref()
}

function terminateChild(child: any, signal: NodeJS.Signals) {
  if (!child?.pid) return

  try {
    if (process.platform === 'win32') {
      spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' })
      return
    }

    process.kill(-child.pid, signal)
  } catch {
    try {
      child.kill(signal)
    } catch {}
  }
}

function cleanupActiveWorkspaces() {
  if (keep) return
  for (const workspace of activeWorkspaces) {
    cleanupWorkspace(workspace)
  }
  activeWorkspaces.clear()
  if (activeRunRoot) {
    cleanupWorkspace(activeRunRoot)
  }
}

function printIntro() {
  console.log('Agent benchmark')
  console.log('')
  console.log('Config')
  console.log(`  model:        ${model}`)
  console.log(`  agents:       ${agents.map(displayAgent).join(', ')}`)
  console.log(`  tasks:        ${selectedTasks.map((task) => task.id).join(', ')}`)
  console.log(`  runs:         ${runs}`)
  console.log(`  prompts:      ${plannedPrompts}`)
  console.log(
    `  timeout:      ${formatDuration(timeoutMs)}${maxRunTimeoutMs === timeoutMs ? '' : ` default, ${formatDuration(maxRunTimeoutMs)} max`}`,
  )
  console.log(`  prompt cost:  ${formatUsd(plannedCost)} estimated`)
  console.log('')
  console.log('Agent model args')
  for (const agent of agents) {
    console.log(`  ${displayAgent(agent).padEnd(12)} ${modelForAgent(agent, model)}`)
  }
  console.log('')
}

function printPaths() {
  console.log('Paths')
  console.log(`  run:         ${runRoot}`)
  console.log(`  workspaces:  ${workspacesRoot}`)
  console.log(`  logs:        ${logsRoot}`)
  if (reportPath) {
    console.log(`  report:      ${reportPath}`)
  }
  console.log('')
  console.log('Notes')
  console.log('  Permission-gated actions are auto-approved for opencode and codex in isolated workspaces.')
  console.log('  Crabcode print mode is run with --dangerously-skip-permissions in isolated workspaces.')
  console.log('  Site-fetch tasks use a per-run 127.0.0.1 static server; they do not hit the public internet.')
  if (!keep) {
    console.log('  Workspaces are removed at exit. Pass --keep to preserve them.')
  }
  console.log('')
}

function printRunStart(runNumber: number, totalRuns: number, agent: AgentName, taskId: string, workspace: string) {
  console.log(`Run ${runNumber}/${totalRuns}: ${displayAgent(agent)} / ${taskId}`)
  console.log(`  workspace: ${workspace}`)
}

function printResult(result: RunResult) {
  const status = result.ok ? 'PASS' : 'FAIL'
  const checks = `${result.passedChecks}/${result.totalChecks}`
  console.log(`  result:    ${status}`)
  console.log(`  checks:    ${checks}`)
  console.log(`  time:      ${formatDuration(result.elapsedMs)}`)
  console.log(`  cost:      ${formatUsd(result.estimatedCostUsd)} estimated`)
  if (result.error) {
    console.log('  reason:')
    for (const line of result.error.split('; ')) {
      console.log(`    - ${line}`)
    }
  }
  if (result.stdoutPath || result.stderrPath) {
    console.log('  output:')
    if (result.stdoutPath) console.log(`    stdout: ${result.stdoutPath}`)
    if (result.stderrPath) console.log(`    stderr: ${result.stderrPath}`)
  }
  console.log('')
}

function printSummary(results: RunResult[]) {
  console.log('\nSummary')
  console.log('| Agent | Score | Checks | Avg time | Est. tokens | Est. cost |')
  console.log('|---|---:|---:|---:|---:|---:|')

  for (const row of summaryRows(results, agents)) {
    console.log(`| ${displayAgent(row.agent)} | ${row.score} | ${row.checks} | ${row.avgTime} | ${row.tokens} | ${row.cost} |`)
  }

  console.log('\nMetric: Score is the percent of task runs where the command exited successfully and every deterministic check passed.')
  console.log('Cost is an estimate from prompt/output text tokens only; provider dashboards are the source of truth.')
}
