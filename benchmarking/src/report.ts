import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { displayAgent, modelForAgent } from './agents.ts'
import { escapeMarkdownTable, formatDuration, formatUsd, sum } from './format.ts'
import { median } from './format.ts'
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
      firstPasses: items.filter((item) => item.firstAttemptOk ?? item.ok).length,
      finalPasses: passCount,
      completedRuns: items.length,
      medianVerifiedMs: median(
        items.flatMap((item) =>
          item.ok &&
          typeof item.verifiedAtMs === 'number' &&
          Number.isFinite(item.verifiedAtMs) &&
          item.verifiedAtMs >= 0
            ? [item.verifiedAtMs]
            : [],
        ),
      ),
      medianAttempts: median(items.flatMap((item) => (item.attempts === undefined ? [] : [item.attempts.length]))),
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
    modelSelection?: Record<string, string>
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
    maxAttempts?: number
    toolProfile?: 'coding' | 'native'
    diagnostics?: boolean
    claudeModel?: string
    claudeEffort?: string
    claudeTools?: string
    reasoningEffort?: string
    parallel?: boolean
  },
) {
  mkdirSync(dirname(path), { recursive: true })
  const lines: string[] = []

  lines.push(`# Agent Benchmark Report`)
  lines.push('')
  lines.push(`Generated: ${new Date().toISOString()}`)
  lines.push(`Run ID: \`${report.runId}\``)
  lines.push(`Model: \`${report.model || '(agent defaults)'}\``)
  const hasModelSelection = report.modelSelection && Object.keys(report.modelSelection).length > 0
  if (hasModelSelection) {
    lines.push(
      `Per-task models: ${Object.entries(report.modelSelection!)
        .map(([id, m]) => `\`${id}=${m}\``)
        .join(', ')}`,
    )
  }
  if (report.claudeModel !== undefined) {
    lines.push(
      `Claude model: \`${report.claudeModel}\` (\`--claude-model\`; independent of the global provider/model ref)`,
    )
  }
  const agentModelArgs = report.agents.flatMap((agent) => {
    const requestedModel =
      agent === 'claude' && report.claudeModel !== undefined
        ? report.claudeModel
        : hasModelSelection
          ? undefined
          : report.model
    return requestedModel && requestedModel !== '(per-task)'
      ? [`\`${displayAgent(agent)}=${modelForAgent(agent, requestedModel)}\``]
      : []
  })
  if (agentModelArgs.length) lines.push(`Agent model args: ${agentModelArgs.join(', ')}`)
  lines.push('Model references and Claude model options do not establish identical provider routes.')
  lines.push(`Agents: ${report.agents.map((agent) => `\`${displayAgent(agent)}\``).join(', ')}`)
  lines.push(`Tasks: ${report.tasks.map((task) => `\`${task.id}\``).join(', ')}`)
  lines.push(`Runs per agent/task: ${report.runs}`)
  lines.push(`Completed runs: ${report.results.length}/${report.plannedPrompts}`)
  if (report.maxAttempts !== undefined) {
    lines.push(`Max attempts per run: ${report.maxAttempts} (\`--max-attempts\`)`)
  }
  if (report.toolProfile !== undefined) {
    lines.push(`Tool profile: \`${report.toolProfile}\` (\`--tool-profile\`)`)
  }
  if (report.diagnostics !== undefined) {
    lines.push(`Diagnostics: ${report.diagnostics ? 'yes' : 'no'} (\`--diagnostics\`)`)
  }
  if (report.claudeEffort !== undefined) {
    lines.push(`Claude effort: \`${report.claudeEffort}\` (\`--claude-effort\`)`)
  }
  if (report.claudeTools !== undefined) {
    lines.push(`Claude tools: \`${report.claudeTools}\` (\`--claude-tools\`)`)
  }
  if (report.reasoningEffort !== undefined) {
    lines.push(`Crabcode reasoning effort: \`${report.reasoningEffort}\` (\`--reasoning-effort\`)`)
  }
  if (report.parallel !== undefined) {
    lines.push(`Parallel: ${report.parallel ? 'yes (timing comparisons are confounded)' : 'no'} (\`--parallel\`)`)
  }
  lines.push(`Default timeout per run: ${report.timeoutMs}ms`)
  const timeoutOverrides = report.tasks
    .filter((task) => task.timeoutMs !== undefined && task.timeoutMs !== report.timeoutMs)
    .map((task) => `${task.id}=${task.timeoutMs}ms`)
  if (timeoutOverrides.length) {
    lines.push(`Task timeout overrides: ${timeoutOverrides.map((override) => `\`${override}\``).join(', ')}`)
  }
  lines.push(`Benchmark run directory: \`${report.runRoot}\``)
  lines.push(`Agents ran in: \`${report.workspacesRoot}\``)
  lines.push(`Logs: \`${report.logsRoot}\``)
  lines.push(`Workspaces kept after run: ${report.keep ? 'yes' : 'no'}`)
  lines.push(`Stopped early: ${report.stopped ? 'yes' : 'no'}`)
  lines.push('')
  lines.push(`Permission-gated actions are auto-approved for benchmark agent commands in isolated workspaces.`)
  lines.push(`Site-fetch tasks use a per-run 127.0.0.1 static server and do not hit the public internet.`)
  lines.push(
    `Cost is a rough estimate from prompt/output text tokens only; provider dashboards are the source of truth.`,
  )
  lines.push('')

  const rows = summaryRows(report.results, report.agents)
  lines.push(`## Verification Summary`)
  lines.push('')
  lines.push(
    '| Agent | First-attempt passes | Final passes | Completed runs | Median time to verified success | Median attempts |',
  )
  lines.push('|---|---:|---:|---:|---:|---:|')
  for (const row of rows) {
    lines.push(
      `| ${displayAgent(row.agent)} | ${row.firstPasses}/${row.completedRuns} | ${row.finalPasses}/${row.completedRuns} | ${row.completedRuns} | ${row.medianVerifiedMs === null ? 'n/a' : formatDuration(row.medianVerifiedMs)} | ${row.medianAttempts ?? 'n/a'} |`,
    )
  }
  lines.push('')
  lines.push(
    'Verified-time medians include successful runs with recorded verification times; attempt medians use recorded attempt arrays across all runs. Unavailable medians are n/a.',
  )
  lines.push('')
  if (report.agents.includes('crabcode') && report.agents.includes('claude')) {
    const paired = pairedSummary(report.results)
    lines.push('## Paired outcomes (Crabcode / Claude)')
    lines.push('')
    lines.push(
      `Both pass: ${paired.bothPassed}; Claude only: ${paired.claudeOnly}; Crabcode only: ${paired.crabcodeOnly}; both fail: ${paired.bothFailed}.`,
    )
    lines.push(
      `Median Crabcode / Claude verified-time ratio on mutually successful pairs: ${paired.medianCrabcodeToClaudeTimeRatio === null ? 'n/a' : paired.medianCrabcodeToClaudeTimeRatio.toFixed(2) + '×'}.`,
    )
    lines.push(
      'Incomplete pairs and unavailable verification times do not contribute timing ratios. Ratios include repair when used and do not establish matched backend behavior.',
    )
    lines.push('')
  }

  lines.push(`## Summary`)
  lines.push('')
  lines.push('The token/cost columns are legacy prompt/output-size estimates—not actual billed tokens.')
  lines.push('')
  lines.push('| Agent | Score | Checks | Avg time | Est. tokens | Est. cost |')
  lines.push('|---|---:|---:|---:|---:|---:|')
  for (const row of rows) {
    lines.push(
      `| ${displayAgent(row.agent)} | ${row.score} | ${row.checks} | ${row.avgTime} | ${row.tokens} | ${row.cost} |`,
    )
  }
  lines.push('')

  lines.push(`## Runs`)
  lines.push('')
  lines.push('The token/cost columns are legacy prompt/output-size estimates—not actual billed tokens.')
  lines.push('')
  lines.push(
    '| Status | Agent | Task | Model | Checks | Time | Est. tokens | Est. cost | Workspace | Stdout | Stderr | Error |',
  )
  lines.push('|---|---|---|---|---:|---:|---:|---:|---|---|---|---|')
  for (const result of report.results) {
    const status = result.ok ? 'PASS' : 'FAIL'
    const tokens = result.estimatedInputTokens + result.estimatedOutputTokens
    lines.push(
      `| ${status} | ${displayAgent(result.agent)} | ${result.task} | \`${result.model ?? ''}\` | ${result.passedChecks}/${result.totalChecks} | ${formatDuration(result.elapsedMs)} | ${tokens} | ${formatUsd(result.estimatedCostUsd)} | \`${result.workspace ?? ''}\` | \`${result.stdoutPath ?? ''}\` | \`${result.stderrPath ?? ''}\` | ${escapeMarkdownTable(result.error ?? '')} |`,
    )
  }
  lines.push('')

  if (report.results.some((result) => result.attempts?.length)) {
    lines.push(`## Attempts`)
    lines.push('')
    lines.push(
      '| Task | Repetition | Agent | Attempt | Status | Checks | Exit code | Timed out (deadline) | Interrupted | Time | Process error | API error | Artifacts |',
    )
    lines.push('|---|---:|---|---:|---|---:|---:|---|---|---:|---|---|---|')
    for (const result of report.results) {
      for (const attempt of result.attempts ?? []) {
        const process = attempt.process
        const passedChecks = attempt.checks.filter((check) => check.pass).length
        const passed =
          attempt.checks.length > 0 &&
          passedChecks === attempt.checks.length &&
          process.exitCode === 0 &&
          !process.timedOut &&
          !process.interrupted &&
          !process.error &&
          !attempt.telemetry?.apiError
        const artifacts = Object.entries(attempt.artifacts)
          .filter(([, path]) => path !== undefined)
          .map(([name, path]) => `${name.replace(/Path$/, '')}=\`${escapeMarkdownTable(path!)}\``)
          .join('; ')
        lines.push(
          `| ${escapeMarkdownTable(result.task)} | ${result.repetition ?? 'n/a'} | ${displayAgent(result.agent)} | ${attempt.number} | ${passed ? 'PASS' : 'FAIL'} | ${passedChecks}/${attempt.checks.length} | ${process.exitCode ?? 'n/a'} | ${process.timedOut ? 'yes' : 'no'} | ${process.interrupted ? 'yes' : 'no'} | ${formatDuration(process.elapsedMs)} | ${escapeMarkdownTable(process.error ?? '')} | ${attempt.telemetry ? (attempt.telemetry.apiError ? 'yes' : 'no') : 'n/a'} | ${artifacts} |`,
        )
      }
    }
    lines.push('')
  }

  const diagnostics = report.results.flatMap((result) => {
    // Run-level telemetry can describe a legacy single invocation, but must not
    // be attributed to a repair attempt when an attempt array is available.
    const attempts = result.attempts ?? (result.telemetry ? [{ number: 1, telemetry: result.telemetry }] : [])
    return attempts.flatMap((attempt) =>
      attempt.telemetry ? [{ result, attempt: attempt.number, telemetry: attempt.telemetry }] : [],
    )
  })
  if (report.diagnostics || diagnostics.length) {
    lines.push(`## Diagnostics (per attempt)`)
    lines.push('')
    lines.push('Recorded telemetry only; n/a means unavailable. No usage is inferred from prose or prompt/output size.')
    lines.push('')
    lines.push(
      '| Task | Repetition | Agent | Attempt | Observed models | Model turns | Tools | Repeats | Reasoning bytes | API ms | Provider ms | Tool ms | Startup ms | Usage input tokens | Usage output tokens | Cache read tokens | Cache write tokens | API error |',
    )
    lines.push('|---|---:|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|')
    for (const { result, attempt, telemetry } of diagnostics) {
      const values = [
        telemetry.modelTurns,
        telemetry.toolCalls,
        telemetry.repeatedToolCalls,
        telemetry.reasoningBytes,
        telemetry.reportedApiMs,
        telemetry.breakdown?.providerMs,
        telemetry.breakdown?.toolMs,
        telemetry.breakdown?.startupMs,
        telemetry.inputTokens,
        telemetry.outputTokens,
        telemetry.cacheReadTokens,
        telemetry.cacheWriteTokens,
      ]
      lines.push(
        `| ${escapeMarkdownTable(result.task)} | ${result.repetition ?? 'n/a'} | ${displayAgent(result.agent)} | ${attempt} | ${telemetry.models.length ? escapeMarkdownTable(telemetry.models.join(', ')) : 'n/a'} | ${values.map((value) => value ?? 'n/a').join(' | ')} | ${telemetry.apiError ? 'yes' : 'no'} |`,
      )
    }
    lines.push('')
  }

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

type PairedRun = Pick<RunResult, 'agent' | 'task' | 'repetition' | 'ok' | 'verifiedAtMs'>

export function pairedSummary(results: PairedRun[]) {
  const groups = new Map<string, Partial<Record<'claude' | 'crabcode', PairedRun>>>()
  for (const result of results) {
    if ((result.agent !== 'claude' && result.agent !== 'crabcode') || result.repetition === undefined) continue
    const key = `${result.task}:${result.repetition}`
    const pair = groups.get(key) || {}
    pair[result.agent] = result
    groups.set(key, pair)
  }
  let bothPassed = 0,
    bothFailed = 0,
    claudeOnly = 0,
    crabcodeOnly = 0
  const ratios: number[] = []
  for (const pair of groups.values()) {
    if (!pair.claude || !pair.crabcode) continue
    if (pair.claude.ok && pair.crabcode.ok) {
      bothPassed++
      const claudeMs = pair.claude.verifiedAtMs
      const crabcodeMs = pair.crabcode.verifiedAtMs
      if (
        typeof claudeMs === 'number' &&
        claudeMs > 0 &&
        Number.isFinite(claudeMs) &&
        typeof crabcodeMs === 'number' &&
        crabcodeMs >= 0 &&
        Number.isFinite(crabcodeMs)
      ) {
        ratios.push(crabcodeMs / claudeMs)
      }
    } else if (pair.claude.ok) claudeOnly++
    else if (pair.crabcode.ok) crabcodeOnly++
    else bothFailed++
  }
  return { bothPassed, bothFailed, claudeOnly, crabcodeOnly, medianCrabcodeToClaudeTimeRatio: median(ratios) }
}
