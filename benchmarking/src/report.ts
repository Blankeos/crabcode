import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { displayAgent, modelForAgent } from './agents.ts'
import { escapeMarkdownTable, formatDuration, formatUsd, sum } from './format.ts'
import type { AgentName, BenchmarkTask, RunResult } from './types.ts'

export function summaryRows(results: RunResult[], agents: AgentName[]) {
  return agents.map((agent) => {
    const items = results.filter((result) => result.agent === agent)
    const passCount = items.filter((result) => result.ok).length
    const totalChecks = sum(items.map((item) => item.totalChecks))
    const passedChecks = sum(items.map((item) => item.passedChecks))
    const avgMs = items.length ? sum(items.map((item) => item.elapsedMs)) / items.length : 0
    const tokens = sum(items.map((item) => item.estimatedInputTokens + item.estimatedOutputTokens))
    const cost = sum(items.map((item) => item.estimatedCostUsd))
    return {
      agent,
      score: items.length ? `${Math.round((passCount / items.length) * 100)}%` : '0%',
      checks: `${passedChecks}/${totalChecks}`,
      avgTime: `${(avgMs / 1000).toFixed(1)}s`,
      tokens,
      cost: formatUsd(cost),
    }
  })
}

export function writeMarkdownReport(
  path: string,
  report: {
    runId: string
    runRoot: string
    workspacesRoot: string
    logsRoot: string
    model: string
    agents: AgentName[]
    tasks: BenchmarkTask[]
    runs: number
    plannedPrompts: number
    timeoutMs: number
    keep: boolean
    inputPrice: number
    outputPrice: number
    results: RunResult[]
    stopped: boolean
  },
) {
  mkdirSync(dirname(path), { recursive: true })
  const lines: string[] = []

  lines.push(`# Agent Benchmark Report`)
  lines.push('')
  lines.push(`Generated: ${new Date().toISOString()}`)
  lines.push(`Run ID: \`${report.runId}\``)
  lines.push(`Model: \`${report.model || '(agent defaults)'}\``)
  lines.push(`Agent model args: ${report.agents.map((agent) => `\`${displayAgent(agent)}=${modelForAgent(agent, report.model)}\``).join(', ')}`)
  lines.push(`Agents: ${report.agents.map((agent) => `\`${displayAgent(agent)}\``).join(', ')}`)
  lines.push(`Tasks: ${report.tasks.map((task) => `\`${task.id}\``).join(', ')}`)
  lines.push(`Runs per agent/task: ${report.runs}`)
  lines.push(`Completed runs: ${report.results.length}/${report.plannedPrompts}`)
  lines.push(`Timeout per run: ${report.timeoutMs}ms`)
  lines.push(`Benchmark run directory: \`${report.runRoot}\``)
  lines.push(`Agents ran in: \`${report.workspacesRoot}\``)
  lines.push(`Logs: \`${report.logsRoot}\``)
  lines.push(`Workspaces kept after run: ${report.keep ? 'yes' : 'no'}`)
  lines.push(`Stopped early: ${report.stopped ? 'yes' : 'no'}`)
  lines.push('')
  lines.push(`Permission-gated actions are auto-approved for benchmark agent commands in isolated workspaces.`)
  lines.push(`Site-fetch tasks use a per-run 127.0.0.1 static server and do not hit the public internet.`)
  lines.push(`Cost is a rough estimate from prompt/output text tokens only; provider dashboards are the source of truth.`)
  lines.push('')

  lines.push(`## Summary`)
  lines.push('')
  lines.push('| Agent | Score | Checks | Avg time | Est. tokens | Est. cost |')
  lines.push('|---|---:|---:|---:|---:|---:|')
  for (const row of summaryRows(report.results, report.agents)) {
    lines.push(`| ${displayAgent(row.agent)} | ${row.score} | ${row.checks} | ${row.avgTime} | ${row.tokens} | ${row.cost} |`)
  }
  lines.push('')

  lines.push(`## Runs`)
  lines.push('')
  lines.push('| Status | Agent | Task | Checks | Time | Est. tokens | Est. cost | Workspace | Stdout | Stderr | Error |')
  lines.push('|---|---|---|---:|---:|---:|---:|---|---|---|---|')
  for (const result of report.results) {
    const status = result.ok ? 'PASS' : 'FAIL'
    const tokens = result.estimatedInputTokens + result.estimatedOutputTokens
    lines.push(
      `| ${status} | ${displayAgent(result.agent)} | ${result.task} | ${result.passedChecks}/${result.totalChecks} | ${formatDuration(result.elapsedMs)} | ${tokens} | ${formatUsd(result.estimatedCostUsd)} | \`${result.workspace ?? ''}\` | \`${result.stdoutPath ?? ''}\` | \`${result.stderrPath ?? ''}\` | ${escapeMarkdownTable(result.error ?? '')} |`,
    )
  }
  lines.push('')

  lines.push(`## Output Tails`)
  lines.push('')
  for (const result of report.results) {
    if (!result.stdoutTail && !result.stderrTail) continue
    lines.push(`### ${displayAgent(result.agent)} / ${result.task}`)
    lines.push('')
    if (result.stdoutTail) {
      lines.push('stdout:')
      lines.push('```text')
      lines.push(result.stdoutTail)
      lines.push('```')
      lines.push('')
    }
    if (result.stderrTail) {
      lines.push('stderr:')
      lines.push('```text')
      lines.push(result.stderrTail)
      lines.push('```')
      lines.push('')
    }
  }

  lines.push(`## Tasks`)
  lines.push('')
  for (const task of report.tasks) {
    lines.push(`### ${task.id}`)
    lines.push('')
    lines.push(task.title)
    lines.push('')
    lines.push('```text')
    lines.push(task.prompt)
    lines.push('```')
    lines.push('')
  }

  writeFileSync(path, lines.join('\n') + '\n')
}

