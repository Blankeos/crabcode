import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, test } from 'bun:test'
import type { Telemetry } from './diagnostics.ts'
import { pairedSummary, summaryRows, writeMarkdownReport } from './report.ts'
import type { BenchmarkAttempt, BenchmarkTask, RunResult } from './types.ts'

const task: BenchmarkTask = { id: 'repair', title: 'Repair', prompt: 'Fix it', files: {}, check: () => [] }
type Report = Parameters<typeof writeMarkdownReport>[1]

function renderReport(overrides: Partial<Report> = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'crabcode-bench-report-'))
  const path = join(dir, 'report.md')
  try {
    writeMarkdownReport(path, {
      runId: 'test',
      runRoot: dir,
      workspacesRoot: join(dir, 'workspaces'),
      logsRoot: join(dir, 'logs'),
      model: 'openai/gpt-5.5',
      agents: ['crabcode'],
      tasks: [task],
      runs: 1,
      plannedPrompts: 1,
      timeoutMs: 45_000,
      keep: false,
      inputPrice: 1.25,
      outputPrice: 10,
      results: [],
      stopped: false,
      ...overrides,
    })
    return readFileSync(path, 'utf8')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function runResult(overrides: Partial<RunResult> = {}): RunResult {
  return {
    agent: 'crabcode',
    task: 'repair',
    ok: true,
    passedChecks: 1,
    totalChecks: 1,
    elapsedMs: 4_000,
    estimatedInputTokens: 70,
    estimatedOutputTokens: 30,
    estimatedCostUsd: 0.02,
    exitCode: 0,
    timedOut: false,
    ...overrides,
  }
}

function attempt(number: number, overrides: Partial<BenchmarkAttempt> = {}): BenchmarkAttempt {
  return {
    number,
    process: { exitCode: 0, timedOut: false, interrupted: false, elapsedMs: 1_000 },
    checks: [{ name: 'fixture', pass: true }],
    artifacts: { stdoutPath: '/logs/stdout', stderrPath: '/logs/stderr', commandPath: '/logs/command' },
    ...overrides,
  }
}

function telemetry(overrides: Partial<Telemetry> = {}): Telemetry {
  return {
    toolCalls: null,
    modelTurns: null,
    reportedApiMs: null,
    reasoningBytes: null,
    toolSequence: null,
    repeatedToolCalls: null,
    breakdown: null,
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reportedCostUsd: null,
    models: [],
    apiError: false,
    ...overrides,
  }
}

function section(markdown: string, title: string) {
  return markdown.split(`## ${title}\n`)[1]?.split('\n## ')[0] ?? ''
}

// Address a few behavioral values by their header, not by Markdown column positions.
function tableRows(markdown: string, title: string) {
  const lines = section(markdown, title).match(/^\|.*$/gm) ?? []
  const cells = (line: string) => line.slice(1, -1).split('|')
  const headers = cells(lines[0])
  return lines
    .slice(2)
    .map((line) => Object.fromEntries(cells(line).map((value, i) => [headers[i].trim(), value.trim()])))
}

test('summary separates first-pass and repaired success, sums estimates and ignores failed/unknown success times', () => {
  const failed = attempt(1, { checks: [{ name: 'fixture', pass: false }] })
  const results = [
    runResult({ verifiedAtMs: 1_000, attempts: [attempt(1)] }),
    runResult({ firstAttemptOk: false, verifiedAtMs: 5_000, attempts: [failed, attempt(2)] }),
    runResult({ ok: false, passedChecks: 0, verifiedAtMs: 100, attempts: [failed] }),
    runResult({ verifiedAtMs: null }),
    runResult(),
  ]
  const [row] = summaryRows(results, ['crabcode'])
  expect(row).toMatchObject({
    firstPasses: 3,
    finalPasses: 4,
    completedRuns: 5,
    medianVerifiedMs: 3_000,
    medianAttempts: 1,
    checks: '4/5',
    tokens: 500,
    cost: '$0.1000',
  })
  expect(summaryRows(results.slice(2), ['crabcode'])[0].medianVerifiedMs).toBeNull()
})

test('paired outcomes exclude incomplete repetitions and failed/unknown timings from ratios', () => {
  const results: Parameters<typeof pairedSummary>[0] = [
    { agent: 'claude', task: 'one', repetition: 1, ok: true, verifiedAtMs: 1_000 },
    { agent: 'crabcode', task: 'one', repetition: 1, ok: true, verifiedAtMs: 2_000 },
    { agent: 'claude', task: 'two', repetition: 1, ok: false, verifiedAtMs: 10 },
    { agent: 'crabcode', task: 'two', repetition: 1, ok: true, verifiedAtMs: 100_000 },
    { agent: 'claude', task: 'one', repetition: 2, ok: true, verifiedAtMs: 1 },
    { agent: 'claude', task: 'unknown', repetition: 1, ok: true, verifiedAtMs: null },
    { agent: 'crabcode', task: 'unknown', repetition: 1, ok: true, verifiedAtMs: 1_000_000 },
  ]
  expect(pairedSummary(results)).toMatchObject({
    bothPassed: 2,
    crabcodeOnly: 1,
    medianCrabcodeToClaudeTimeRatio: 2,
  })
  expect(pairedSummary(results.slice(4)).medianCrabcodeToClaudeTimeRatio).toBeNull()
})

test('report keeps the global model distinct from the normalized Claude override and retains task timeouts', () => {
  const markdown = renderReport({
    agents: ['crabcode', 'claude'],
    claudeModel: 'anthropic/claude-sonnet-5-5',
    tasks: [{ ...task, timeoutMs: 180_000 }],
  })
  expect(markdown).toContain('Model: `openai/gpt-5.5`')
  expect(markdown).toContain('Claude model: `anthropic/claude-sonnet-5-5`')
  expect(markdown).toContain('crabcode=openai/gpt-5.5')
  expect(markdown).toContain('claude=claude-sonnet-5-5')
  expect(markdown).not.toContain('claude=gpt-5.5')
  expect(markdown).toContain('Task timeout overrides: `repair=180000ms`')
})

test('trace reporting uses measured usage, preserves zero/unknown values and artifact paths, not prose or estimates', () => {
  const traced = attempt(1, {
    telemetry: telemetry({
      models: ['observed-model'],
      toolCalls: 0,
      inputTokens: 54_321,
      outputTokens: 432,
      cacheReadTokens: 0,
    }),
  })
  traced.artifacts.tracePath = '/logs/trace.jsonl'
  traced.artifacts.stepsPath = '/logs/steps.json'
  const markdown = renderReport({
    diagnostics: true,
    results: [
      runResult({
        estimatedInputTokens: 900_001,
        estimatedOutputTokens: 900_002,
        stdoutTail: 'I used 12345678 tokens.',
        telemetry: telemetry({ inputTokens: 999_999 }),
        attempts: [traced, attempt(2, { telemetry: telemetry() })],
      }),
    ],
  })
  const [measured, unknown] = tableRows(markdown, 'Diagnostics (per attempt)')
  expect(measured).toMatchObject({
    'Observed models': 'observed-model',
    Tools: '0',
    'Usage input tokens': '54321',
    'Usage output tokens': '432',
    'Cache read tokens': '0',
    'Cache write tokens': 'n/a',
  })
  expect(unknown).toMatchObject({ Tools: 'n/a', 'Usage input tokens': 'n/a' })
  const diagnostics = section(markdown, 'Diagnostics (per attempt)')
  for (const value of ['900001', '900002', '1800003', '12345678', '999999']) {
    expect(diagnostics).not.toContain(value)
  }
  expect(tableRows(markdown, 'Summary')[0]['Est. tokens']).toBe('1800003')
  expect(section(markdown, 'Summary')).toContain('not actual billed tokens')
  for (const path of Object.values(traced.artifacts)) expect(section(markdown, 'Attempts')).toContain(path)
})
