import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, test } from 'bun:test'
import { writeMarkdownReport } from './report.ts'
import type { BenchmarkTask } from './types.ts'

test('markdown report shows task timeout overrides separately from the default timeout', () => {
  const dir = mkdtempSync(join(tmpdir(), 'crabcode-bench-report-'))
  const reportPath = join(dir, 'report.md')
  const tasks: BenchmarkTask[] = [
    {
      id: 'issue-triage-pipeline-ts',
      title: 'Implement triage',
      prompt: 'Implement triage.',
      timeoutMs: 180_000,
      files: {},
      check: () => [],
    },
  ]

  writeMarkdownReport(reportPath, {
    runId: 'test-run',
    runRoot: dir,
    workspacesRoot: join(dir, 'workspaces'),
    logsRoot: join(dir, 'logs'),
    model: 'openai/gpt-5.5',
    agents: ['crabcode'],
    tasks,
    runs: 1,
    plannedPrompts: 1,
    timeoutMs: 45_000,
    keep: false,
    inputPrice: 1.25,
    outputPrice: 10,
    results: [],
    stopped: false,
  })

  const markdown = readFileSync(reportPath, 'utf8')
  expect(markdown).toContain('Default timeout per run: 45000ms')
  expect(markdown).toContain('Task timeout overrides: `issue-triage-pipeline-ts=180000ms`')
})
