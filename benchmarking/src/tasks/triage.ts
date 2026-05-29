import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { bunTestWithHiddenFileCheck } from '../checks.ts'
import { defineTask } from './define.ts'

export const triageTasks = [
  defineTask({
    id: 'issue-triage-pipeline-ts',
    title: 'Implement a multi-file issue triage pipeline',
    difficulty: 'hard',
    tags: ['typescript', 'cli', 'multi-file', 'hidden-tests'],
    timeoutMs: 120_000,
    files: {
      'package.json': JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n',
      'README.md': `# Triage Pipeline Fixture

Implement the triage pipeline without adding dependencies.

Scoring:
- p0/p1/p2/p3 severities are worth 100/70/40/10.
- security and data-loss labels add 25 points each.
- customer adds 15, regression adds 10, and low-priority subtracts 15.
- stale days are the full days between updatedAt and asOf, capped at 30.
- blocked marks an issue as blocked; blocked issues sort after ready issues in the same owner group.

Sorting:
- Groups sort by total score descending, then owner alphabetically, with unassigned last.
- Issues sort by blocked status, score descending, severity, updatedAt oldest first, then id.

Markdown:
- Start with "# Issue Triage - YYYY-MM-DD".
- Include "Open issues: N" and "Top issue: ID (score)".
- Each owner section is "## owner (N issues, score S)".
- Each issue line is "- [score] ID severity ready|blocked - title".
- Include a following "  Labels: label, label" line when labels exist.
`,
      'src/types.ts': `export type Severity = 'p0' | 'p1' | 'p2' | 'p3'

export type IssueInput = {
  id: string
  title: string
  severity: string
  labels?: string[]
  owner?: string | null
  status?: string
  createdAt?: string
  updatedAt: string
}

export type RankedIssue = {
  id: string
  title: string
  severity: Severity
  labels: string[]
  owner: string
  status: 'open'
  createdAt?: string
  updatedAt: string
  score: number
  blocked: boolean
}

export type TriageGroup = {
  owner: string
  issues: RankedIssue[]
  totalScore: number
}

export type TriagePlan = {
  asOf: string
  totalOpen: number
  groups: TriageGroup[]
  topIssue: RankedIssue | null
}

export type PlanOptions = {
  asOf?: string
}
`,
      'src/scoring.ts': `import type { IssueInput, Severity } from './types'

export const severityOrder: Record<Severity, number> = {
  p0: 0,
  p1: 1,
  p2: 2,
  p3: 3,
}

export function normalizeSeverity(value: string): Severity {
  const normalized = value.toLowerCase()
  if (normalized === 'p0' || normalized === 'p1' || normalized === 'p2' || normalized === 'p3') {
    return normalized
  }
  return 'p3'
}

export function scoreIssue(issue: Pick<IssueInput, 'severity' | 'labels' | 'updatedAt'>, asOf: string): number {
  return 0
}
`,
      'src/triage.ts': `import { normalizeSeverity, scoreIssue, severityOrder } from './scoring'
import type { IssueInput, PlanOptions, RankedIssue, TriagePlan } from './types'

export function normalizeIssues(issues: IssueInput[]): RankedIssue[] {
  return issues
    .filter((issue) => issue.status !== 'closed')
    .map((issue) => ({
      id: issue.id,
      title: issue.title,
      severity: normalizeSeverity(issue.severity),
      labels: issue.labels ?? [],
      owner: issue.owner ?? 'unassigned',
      status: 'open',
      createdAt: issue.createdAt,
      updatedAt: issue.updatedAt,
      score: 0,
      blocked: false,
    }))
}

export function createTriagePlan(issues: IssueInput[], options: PlanOptions = {}): TriagePlan {
  const asOf = options.asOf ?? new Date().toISOString().slice(0, 10)
  const normalized = normalizeIssues(issues).map((issue) => ({
    ...issue,
    score: scoreIssue(issue, asOf),
    blocked: issue.labels.includes('blocked'),
  }))

  return {
    asOf,
    totalOpen: normalized.length,
    groups: [],
    topIssue: normalized[0] ?? null,
  }
}

export { severityOrder }
`,
      'src/report.ts': `import type { TriagePlan } from './types'

export function renderTriageMarkdown(plan: TriagePlan): string {
  return JSON.stringify(plan, null, 2)
}
`,
      'src/cli.ts': `import { readFileSync } from 'node:fs'
import { createTriagePlan } from './triage'
import { renderTriageMarkdown } from './report'
import type { IssueInput } from './types'

const [, , filePath, ...args] = process.argv
const asOfIndex = args.indexOf('--as-of')
const asOf = asOfIndex >= 0 ? args[asOfIndex + 1] : undefined

if (!filePath) {
  console.error('Usage: bun src/cli.ts issues.json [--as-of YYYY-MM-DD]')
  process.exit(1)
}

const issues = JSON.parse(readFileSync(filePath, 'utf8')) as IssueInput[]
console.log(renderTriageMarkdown(createTriagePlan(issues, { asOf })))
`,
      'src/index.ts': `export * from './types'
export * from './scoring'
export * from './triage'
export * from './report'
`,
      'tests/triage.test.ts': `import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { expect, test } from 'bun:test'
import { createTriagePlan, normalizeIssues, renderTriageMarkdown, scoreIssue, type IssueInput } from '../src/index'

const sampleIssues: IssueInput[] = [
  {
    id: 'PAY-9',
    title: ' Card failures ',
    severity: 'P1',
    labels: ['Customer', 'Regression'],
    owner: 'Payments',
    status: 'open',
    updatedAt: '2026-05-10',
  },
  {
    id: 'PAY-9',
    title: 'Old duplicate',
    severity: 'p3',
    labels: [],
    owner: 'payments',
    status: 'open',
    updatedAt: '2026-05-01',
  },
  {
    id: 'PAY-7',
    title: 'Blocked webhook',
    severity: 'p1',
    labels: ['blocked'],
    owner: 'payments',
    status: 'open',
    updatedAt: '2026-05-01',
  },
  {
    id: 'AUTH-1',
    title: 'Token leak report',
    severity: 'p2',
    labels: ['security'],
    owner: 'auth',
    status: 'open',
    updatedAt: '2026-05-18',
  },
  {
    id: 'DOC-4',
    title: ' docs typo ',
    severity: 'p3',
    labels: ['LOW-PRIORITY'],
    status: 'open',
    updatedAt: '2026-04-01',
  },
  {
    id: 'DONE-1',
    title: 'Already shipped',
    severity: 'p0',
    labels: ['customer'],
    owner: 'payments',
    status: 'closed',
    updatedAt: '2026-05-19',
  },
]

test('normalizes open issues and keeps the latest duplicate', () => {
  const normalized = normalizeIssues(sampleIssues).map(({ id, title, severity, labels, owner, status, updatedAt, score, blocked }) => ({
    id,
    title,
    severity,
    labels,
    owner,
    status,
    updatedAt,
    score,
    blocked,
  }))

  expect(normalized).toEqual([
    {
      id: 'PAY-9',
      title: 'Card failures',
      severity: 'p1',
      labels: ['customer', 'regression'],
      owner: 'payments',
      status: 'open',
      updatedAt: '2026-05-10',
      score: 0,
      blocked: false,
    },
    {
      id: 'PAY-7',
      title: 'Blocked webhook',
      severity: 'p1',
      labels: ['blocked'],
      owner: 'payments',
      status: 'open',
      updatedAt: '2026-05-01',
      score: 0,
      blocked: false,
    },
    {
      id: 'AUTH-1',
      title: 'Token leak report',
      severity: 'p2',
      labels: ['security'],
      owner: 'auth',
      status: 'open',
      updatedAt: '2026-05-18',
      score: 0,
      blocked: false,
    },
    {
      id: 'DOC-4',
      title: 'docs typo',
      severity: 'p3',
      labels: ['low-priority'],
      owner: 'unassigned',
      status: 'open',
      updatedAt: '2026-04-01',
      score: 0,
      blocked: false,
    },
  ])
})

test('scores, groups, and orders issues for triage', () => {
  expect(scoreIssue(sampleIssues[0], '2026-05-20')).toBe(105)

  const plan = createTriagePlan(sampleIssues, { asOf: '2026-05-20' })

  expect(plan.totalOpen).toBe(4)
  expect(plan.groups.map((group) => group.owner)).toEqual(['payments', 'auth', 'unassigned'])
  expect(plan.groups[0].totalScore).toBe(194)
  expect(plan.groups[0].issues.map((issue) => \`\${issue.id}:\${issue.score}:\${issue.blocked}\`)).toEqual([
    'PAY-9:105:false',
    'PAY-7:89:true',
  ])
  expect(plan.topIssue?.id).toBe('PAY-9')
})

test('renders the deterministic markdown report', () => {
  const markdown = renderTriageMarkdown(createTriagePlan(sampleIssues, { asOf: '2026-05-20' }))

  expect(markdown).toContain('# Issue Triage - 2026-05-20')
  expect(markdown).toContain('Open issues: 4')
  expect(markdown).toContain('Top issue: PAY-9 (105)')
  expect(markdown).toContain('## payments (2 issues, score 194)')
  expect(markdown).toContain('- [105] PAY-9 p1 ready - Card failures')
  expect(markdown).toContain('  Labels: customer, regression')
  expect(markdown).toContain('- [89] PAY-7 p1 blocked - Blocked webhook')
})

test('CLI reads a JSON file and renders markdown', () => {
  const dir = mkdtempSync(join(tmpdir(), 'triage-bench-'))
  const input = join(dir, 'issues.json')
  writeFileSync(input, JSON.stringify(sampleIssues))

  const result = spawnSync(process.execPath, ['src/cli.ts', input, '--as-of', '2026-05-20'], {
    cwd: process.cwd(),
    encoding: 'utf8',
  })

  expect(result.status).toBe(0)
  expect(result.stderr).toBe('')
  expect(result.stdout).toContain('# Issue Triage - 2026-05-20')
  expect(result.stdout).toContain('PAY-9')
})
`,
    },
    prompt: `Implement the issue triage pipeline described in README.md. You will need to update the TypeScript modules under src/ so normalization, scoring, grouping, Markdown rendering, and the CLI all work together.

Requirements:
- Do not change tests or add dependencies.
- Deduplicate issues by id before filtering; when duplicates exist, keep the issue with the latest updatedAt.
- Treat missing or non-open status as open, but remove issues whose latest status is closed.
- Normalize titles by trimming whitespace; normalize owners and labels to lowercase; default missing owner to unassigned.
- Score with the README rules, using full stale days from updatedAt to asOf and a 30 day cap.
- Build owner groups with total scores, sorted as described in README.md.
- Mark blocked issues from the blocked label and sort blocked issues after ready issues in the same group.
- Set topIssue to the highest-priority ready issue across the full plan, falling back to the highest-priority blocked issue only when every issue is blocked.
- renderTriageMarkdown must follow the README Markdown format and include "No open issues" for an empty plan.
- src/cli.ts must read the JSON input file, support --as-of YYYY-MM-DD, print the Markdown report, and exit with a non-zero code plus a useful error on invalid input.`,
    check: (cwd) => {
      const testFile = readFileSync(join(cwd, 'tests/triage.test.ts'), 'utf8')
      return [
        {
          name: 'keeps visible triage coverage',
          pass:
            testFile.includes('normalizes open issues') &&
            testFile.includes('scores, groups, and orders issues') &&
            testFile.includes('CLI reads a JSON file'),
        },
        bunTestWithHiddenFileCheck(
          cwd,
          'hidden triage pipeline tests pass',
          'tests/__bench_hidden_triage.test.ts',
          `import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { expect, test } from 'bun:test'
import { createTriagePlan, normalizeIssues, renderTriageMarkdown, type IssueInput } from '../src/index'

test('latest closed duplicate removes the issue from the plan', () => {
  const issues: IssueInput[] = [
    { id: 'A', title: 'open first', severity: 'p0', status: 'open', updatedAt: '2026-05-01' },
    { id: 'A', title: 'closed later', severity: 'p0', status: 'closed', updatedAt: '2026-05-02' },
    { id: 'B', title: 'still open', severity: 'p2', labels: ['customer'], updatedAt: '2026-05-03' },
  ]

  expect(normalizeIssues(issues).map((issue) => issue.id)).toEqual(['B'])
  expect(createTriagePlan(issues, { asOf: '2026-05-10' }).totalOpen).toBe(1)
})

test('ready issues sort before blocked issues even when blocked scores higher', () => {
  const plan = createTriagePlan(
    [
      { id: 'B', title: 'blocked critical', severity: 'p0', labels: ['blocked', 'security'], owner: 'Ops', updatedAt: '2026-05-01' },
      { id: 'C', title: 'ready same score', severity: 'p2', owner: 'ops', updatedAt: '2026-05-09' },
      { id: 'A', title: 'ready same score', severity: 'p2', owner: 'ops', updatedAt: '2026-05-09' },
    ],
    { asOf: '2026-05-10' },
  )

  expect(plan.groups[0].owner).toBe('ops')
  expect(plan.groups[0].issues.map((issue) => issue.id)).toEqual(['A', 'C', 'B'])
  expect(plan.topIssue?.id).toBe('A')
})

test('empty reports stay useful and the CLI reports invalid JSON', () => {
  const markdown = renderTriageMarkdown(createTriagePlan([], { asOf: '2026-05-20' }))
  expect(markdown).toContain('Open issues: 0')
  expect(markdown).toContain('No open issues')

  const dir = mkdtempSync(join(tmpdir(), 'triage-hidden-'))
  const input = join(dir, 'broken.json')
  writeFileSync(input, '{not json')
  const result = spawnSync(process.execPath, ['src/cli.ts', input], {
    cwd: process.cwd(),
    encoding: 'utf8',
  })

  expect(result.status).not.toBe(0)
  expect(result.stderr).toMatch(/invalid|json|parse/i)
})
`,
        ),
      ]
    },
  }),
]
