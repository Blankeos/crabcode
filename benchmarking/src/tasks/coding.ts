// Small, dependency-free coding fixtures for harness comparisons.
// Only `files` goes into the candidate workspace. `checks` stays with the clean-room grader.
import type { GradedFixture } from '../types.ts'

const packageJson = JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n'

const workspaceRules = `Work only in your own provided workspace. Use Bun; no dependencies or installs are needed.
Keep the visible tests and public exports. Run bun test (or bun run test) before finishing.
The grader copies source modules into a clean room and supplies immutable visible and hidden tests;
changes to tests, package scripts, documentation, or configuration cannot change the grade.`

const paginationVisible = `import { expect, test } from 'bun:test'
import { page } from '../src/page.ts'

test('takes a bounded page at a nonzero offset', () => {
  expect(page(['a', 'b', 'c', 'd', 'e'], 2, 2)).toEqual(['c', 'd'])
})

test('zero limit is empty and the final page can be short', () => {
  expect(page([1, 2, 3], 1, 0)).toEqual([])
  expect(page([1, 2, 3], 2, 5)).toEqual([3])
})

test('rejects negative offsets and fractional limits', () => {
  expect(() => page([1], -1, 1)).toThrow(RangeError)
  expect(() => page([1], 0, 0.5)).toThrow(RangeError)
})
`

const statusVisible = `import { expect, test } from 'bun:test'
import { STATUS_ORDER, statusLabel, type JobStatus } from '../src/status.ts'
import { summarizeJobs } from '../src/summary.ts'

test('exposes blocked between queued and running', () => {
  expect(STATUS_ORDER).toEqual(['queued', 'blocked', 'running', 'done'])
  expect(statusLabel('blocked' as JobStatus)).toBe('Blocked')
})

test('counts blocked jobs and preserves report line order', () => {
  expect(summarizeJobs([
    { id: 'b', status: 'blocked' as JobStatus },
    { id: 'a', status: 'done' },
    { id: 'c', status: 'queued' },
  ])).toEqual({
    counts: { queued: 1, blocked: 1, running: 0, done: 1 },
    lines: ['b: Blocked', 'a: Done', 'c: Queued'],
  })
})

test('keeps labels for the existing statuses', () => {
  expect(['queued', 'running', 'done'].map((status) => statusLabel(status as JobStatus)))
    .toEqual(['Queued', 'Running', 'Done'])
})
`

const billingVisible = `import { expect, test } from 'bun:test'
import { billableBlocks } from '../src/billing/blocks.ts'
import { storageCost, transferCost } from '../src/billing/costs.ts'

test('shared helper counts partial blocks and included usage', () => {
  expect(billableBlocks(12, 2, 5)).toBe(2)
  expect(billableBlocks(12.1, 2, 5)).toBe(3)
  expect(billableBlocks(1, 2, 5)).toBe(0)
})

test('preserves storage and transfer charges', () => {
  expect(storageCost(12, 2, 7)).toBe(14)
  expect(storageCost(12.1, 2, 7)).toBe(21)
  expect(transferCost(22, 2, 3)).toBe(6)
  expect(transferCost(22.1, 2, 3)).toBe(9)
  expect(storageCost(0, 5, 7)).toBe(0)
  expect(transferCost(4, 4, 3)).toBe(0)
})
`

const redirectsVisible = `import { expect, test } from 'bun:test'
import { resolveRedirect } from '../packages/router/src/redirects.ts'

// Deliberate deterministic noise: these are not test failures or network requests.
for (let index = 1; index <= 24; index++) {
  console.warn('[fixture warning ' + index + '] optional telemetry is disabled; continuing offline')
}

test('keeps unmapped routes and single-hop redirects', () => {
  expect(resolveRedirect('/home', {})).toBe('/home')
  expect(resolveRedirect('/old', { '/old': '/new' })).toBe('/new')
})

test('follows a redirect chain to its terminal route', () => {
  expect(resolveRedirect('/old', { '/old': '/middle', '/middle': '/new' })).toBe('/new')
})

test('returns the original request if a cycle is reachable', () => {
  expect(resolveRedirect('/start', {
    '/start': '/a', '/a': '/b', '/b': '/a',
  })).toBe('/start')
})
`

const archiveVisible = `import { expect, test } from 'bun:test'
import { resolveRedirect } from '../tools/archive/redirects.ts'

test('keeps the unrelated archive naming behavior', () => {
  expect(resolveRedirect('old')).toBe('archive/old')
  expect(resolveRedirect('')).toBe('archive/')
})
`

export const codingTasks: GradedFixture[] = [
  {
    id: 'page-boundaries',
    title: 'Fix pagination boundary cases',
    prompt: `Fix page<T>(items, offset, limit) in src/page.ts.
The input is a readonly array. Return a new array of at most limit items starting at the
zero-based offset, in input order, without mutating the input or cloning its elements.
A zero limit, empty input, or offset at/beyond the end returns []. A short last page
contains only the remaining items.
Both offset and limit must be finite nonnegative integers; otherwise throw RangeError,
even when the input is empty or the limit is zero. No other input validation is required.
${workspaceRules}`,
    files: {
      'package.json': packageJson,
      'README.md': `# Page utility

Run \`bun test\` or \`bun run test\` from this workspace. No install is needed.
The exported page utility lives in src/page.ts; tests live in tests/.
`,
      'src/page.ts': `export function page<T>(items: readonly T[], offset: number, limit: number): T[] {
  return items.slice(offset, limit)
}
`,
      'tests/page.test.ts': paginationVisible,
    },
    checks: {
      'tests/page.test.ts': paginationVisible,
      'tests/page.hidden.test.ts': `import { expect, test } from 'bun:test'
import { page } from '../src/page.ts'

test('handles empty, full, short and out-of-range pages', () => {
  for (const [offset, limit, expected] of [
    [0, 0, []], [0, 2, [10, 20]], [1, 2, [20, 30]],
    [3, 1, [40]], [4, 2, []], [50, 2, []], [0, 50, [10, 20, 30, 40]],
  ] as const) {
    expect(page([10, 20, 30, 40], offset, limit)).toEqual(expected)
  }
  expect(page([], 0, 4)).toEqual([])
})

test('validates both arguments before handling empty pages', () => {
  for (const invalid of [-1, -0.1, 0.5, NaN, Infinity, -Infinity]) {
    expect(() => page([], invalid, 0)).toThrow(RangeError)
    expect(() => page([], 0, invalid)).toThrow(RangeError)
  }
})

test('returns a fresh array without mutating or cloning elements', () => {
  const first = { id: 'a' }
  const second = { id: 'b' }
  const input = Object.freeze([first, second])
  const output = page(input, 0, 10)
  expect(output).not.toBe(input)
  expect(output[0]).toBe(first)
  expect(output[1]).toBe(second)
  output.pop()
  expect(input).toEqual([first, second])
})
`,
    },
  },
  {
    id: 'job-status',
    title: 'Add a job status across the reporting modules',
    prompt: `Add the job status 'blocked' throughout src/status.ts and src/summary.ts.
JobStatus must include queued, blocked, running and done. Export STATUS_ORDER in exactly
that order. statusLabel returns Queued, Blocked, Running or Done for its respective status.
summarizeJobs accepts a readonly array of { id: string, status: JobStatus } and returns
{ counts, lines }. counts contains exactly those four status keys, including zero counts.
Each job contributes once, even if ids repeat. lines contains one 'id: Label' string per
job in the original order. Empty input gives all zero counts and no lines. Do not mutate
input jobs, arrays or STATUS_ORDER; returned counts and lines must be fresh on each call.
Preserve the existing statuses. Inputs contain only valid statuses; no runtime validation
is required.
${workspaceRules}`,
    files: {
      'package.json': packageJson,
      'README.md': `# Job report

Run \`bun test\` or \`bun run test\` here without installing anything.
src/status.ts owns status metadata; src/summary.ts produces reports.
`,
      'src/status.ts': `export type JobStatus = 'queued' | 'running' | 'done'

export const STATUS_ORDER: readonly JobStatus[] = ['queued', 'running', 'done']

export function statusLabel(status: JobStatus): string {
  const labels: Record<JobStatus, string> = {
    queued: 'Queued',
    running: 'Running',
    done: 'Done',
  }
  return labels[status]
}
`,
      'src/summary.ts': `import { statusLabel, type JobStatus } from './status.ts'

export type Job = { id: string, status: JobStatus }

export function summarizeJobs(jobs: readonly Job[]): { counts: Record<JobStatus, number>, lines: string[] } {
  const counts: Record<JobStatus, number> = { queued: 0, running: 0, done: 0 }
  const lines: string[] = []
  for (const job of jobs) {
    counts[job.status] += 1
    lines.push(job.id + ': ' + statusLabel(job.status))
  }
  return { counts, lines }
}
`,
      'tests/status.test.ts': statusVisible,
    },
    checks: {
      'tests/status.test.ts': statusVisible,
      'tests/status.hidden.test.ts': `import { expect, test } from 'bun:test'
import { STATUS_ORDER, statusLabel, type JobStatus } from '../src/status.ts'
import { summarizeJobs } from '../src/summary.ts'

test('includes all zero counts for empty input', () => {
  expect(summarizeJobs([])).toEqual({
    counts: { queued: 0, blocked: 0, running: 0, done: 0 }, lines: [],
  })
})

test('counts repeated ids and preserves frozen jobs and metadata', () => {
  const jobs = Object.freeze([
    Object.freeze({ id: 'same', status: 'blocked' as JobStatus }),
    Object.freeze({ id: 'same', status: 'running' as JobStatus }),
    Object.freeze({ id: 'same', status: 'blocked' as JobStatus }),
    Object.freeze({ id: '', status: 'done' as JobStatus }),
  ])
  const before = structuredClone(jobs)
  const order = [...STATUS_ORDER]
  expect(summarizeJobs(jobs)).toEqual({
    counts: { queued: 0, blocked: 2, running: 1, done: 1 },
    lines: ['same: Blocked', 'same: Running', 'same: Blocked', ': Done'],
  })
  expect(jobs).toEqual(before)
  expect(STATUS_ORDER).toEqual(order)
})

test('returns independent reports and handles every status', () => {
  const jobs = ['queued', 'blocked', 'running', 'done'].map((status) => ({
    id: status, status: status as JobStatus,
  }))
  const first = summarizeJobs(jobs)
  first.counts.queued = 99
  first.lines.push('extra')
  expect(summarizeJobs(jobs)).toEqual({
    counts: { queued: 1, blocked: 1, running: 1, done: 1 },
    lines: ['queued: Queued', 'blocked: Blocked', 'running: Running', 'done: Done'],
  })
  for (const job of jobs) expect(statusLabel(job.status)).toBe(first.lines[jobs.indexOf(job)].split(': ')[1])
})
`,
    },
  },
  {
    id: 'billing-refactor',
    title: 'Extract shared billing logic without changing charges',
    prompt: `Refactor src/billing/costs.ts to share its block calculation via the exported
billableBlocks(used, included, blockSize) helper in src/billing/blocks.ts. Implement that
helper: usage up to included is free; excess usage consumes whole blocks, with any partial
block billed as one whole block. Return the number of billable blocks, not a monetary amount.
Both storageCost and transferCost must delegate their block calculation to this helper
with their original used/included arguments and block sizes 5 and 10 respectively, then
multiply by centsPerBlock. Preserve their existing exported names, argument order and numeric
results. Do not round the monetary result. used and included are finite nonnegative numbers
(fractions allowed), blockSize is finite and positive (fractions allowed), and centsPerBlock
is finite and nonnegative (fractions allowed). No validation of other inputs is required.
${workspaceRules}`,
    files: {
      'package.json': packageJson,
      'README.md': `# Billing refactor

Run \`bun test\` or \`bun run test\` here. No dependencies are required.
The existing cost functions are in src/billing/costs.ts; blocks.ts is the new helper boundary.
`,
      'src/billing/blocks.ts': `export function billableBlocks(used: number, included: number, blockSize: number): number {
  // The shared helper is not implemented yet.
  return 0
}
`,
      'src/billing/costs.ts': `export function storageCost(used: number, included: number, centsPerBlock: number): number {
  const excess = Math.max(0, used - included)
  return Math.ceil(excess / 5) * centsPerBlock
}

export function transferCost(used: number, included: number, centsPerBlock: number): number {
  const excess = Math.max(0, used - included)
  return Math.ceil(excess / 10) * centsPerBlock
}
`,
      'tests/billing.test.ts': billingVisible,
    },
    checks: {
      'tests/billing.test.ts': billingVisible,
      'tests/billing.hidden.test.ts': `import { expect, spyOn, test } from 'bun:test'
import * as blocks from '../src/billing/blocks.ts'
import { storageCost, transferCost } from '../src/billing/costs.ts'

test('handles exact boundaries, zero usage and fractional block sizes', () => {
  for (const [used, included, size, expected] of [
    [0, 0, 5, 0], [5, 5, 5, 0], [3, 8, 2, 0],
    [10, 0, 5, 2], [10.25, 0, 5, 3], [1.25, 0.5, 0.25, 3],
  ]) {
    expect(blocks.billableBlocks(used, included, size)).toBe(expected)
  }
})

test('keeps legacy charge behavior over a deterministic input grid', () => {
  for (const used of [0, 0.25, 5, 10, 10.25, 27]) {
    for (const included of [0, 0.5, 10]) {
      for (const price of [0, 0.25, 7]) {
        const excess = Math.max(0, used - included)
        expect(storageCost(used, included, price)).toBe(Math.ceil(excess / 5) * price)
        expect(transferCost(used, included, price)).toBe(Math.ceil(excess / 10) * price)
      }
    }
  }
})

test('both existing cost functions delegate block calculation to the shared helper', () => {
  const helper = spyOn(blocks, 'billableBlocks')
  try {
    helper.mockReturnValue(4)
    expect(storageCost(12, 2, 7)).toBe(28)
    expect(helper).toHaveBeenLastCalledWith(12, 2, 5)
    expect(transferCost(22, 3, 0.5)).toBe(2)
    expect(helper).toHaveBeenLastCalledWith(22, 3, 10)
  } finally {
    helper.mockRestore()
  }
})
`,
    },
  },
  {
    id: 'noisy-redirects',
    title: 'Recover from noisy tests and fix nested redirect routing',
    prompt: `Fix the active resolveRedirect(path, redirects) export used by the failing router
tests. This small repository has a nested router package and an unrelated archive utility;
use the README and test imports to find the active implementation, not the similarly named
archive file. The telemetry warnings in test output are expected offline noise.
Follow redirects transitively until the current path has no own property in the redirect
map, then return that terminal path. If any cycle is reachable (including a self-redirect),
return the original requested path. Chains can have any finite length, so an arbitrary hop
limit must not truncate them. Paths and targets are case-sensitive strings; the empty string
is a valid path/target. Only own properties count, never inherited properties. The map has
string values and may have a null prototype. Do not mutate it. Keep the public API and the
unrelated archive utility's existing behavior.
${workspaceRules}`,
    files: {
      'package.json': packageJson,
      'README.md': `# Router workspace

Run \`bun test\` or \`bun run test\` from this root; no install or network is needed.

## Repository map

- packages/router/src/index.ts: public router entry point
- packages/router/src/redirects.ts: active redirect resolution
- tools/archive/redirects.ts: an unrelated archive-name utility, not part of the router
- tests/router.test.ts: router regressions; prints expected telemetry warnings
- tests/archive.test.ts: archive smoke test

The numbered telemetry warnings are harmless fixture output. Read the failing assertions below them.
`,
      'packages/router/src/index.ts': `export { resolveRedirect } from './redirects.ts'
`,
      'packages/router/src/redirects.ts': `export function resolveRedirect(path: string, redirects: Readonly<Record<string, string>>): string {
  return redirects[path] ?? path
}
`,
      'tools/archive/redirects.ts': `// Archive naming is separate from HTTP routing.
export function resolveRedirect(name: string): string {
  return 'archive/' + name
}
`,
      'tests/router.test.ts': redirectsVisible,
      'tests/archive.test.ts': archiveVisible,
    },
    checks: {
      'tests/router.test.ts': redirectsVisible,
      'tests/archive.test.ts': archiveVisible,
      'tests/router.hidden.test.ts': `import { expect, test } from 'bun:test'
import { resolveRedirect } from '../packages/router/src/index.ts'

test('handles self cycles, cycles through the start, and cycles after a prefix', () => {
  expect(resolveRedirect('/a', { '/a': '/a' })).toBe('/a')
  expect(resolveRedirect('/a', { '/a': '/b', '/b': '/a' })).toBe('/a')
  expect(resolveRedirect('/entry', {
    '/entry': '/a', '/a': '/b', '/b': '/c', '/c': '/b',
  })).toBe('/entry')
})

test('supports long chains without mutating a frozen map', () => {
  const map: Record<string, string> = {}
  for (let index = 0; index < 128; index++) map['/' + index] = '/' + (index + 1)
  const before = { ...map }
  Object.freeze(map)
  expect(resolveRedirect('/0', map)).toBe('/128')
  expect(map).toEqual(before)
})

test('preserves case and accepts empty targets and empty keys', () => {
  expect(resolveRedirect('/A', { '/a': '/b' })).toBe('/A')
  expect(resolveRedirect('/a', { '/a': '' })).toBe('')
  expect(resolveRedirect('/a', { '/a': '', '': '/end' })).toBe('/end')
  expect(resolveRedirect('', { '': '/end' })).toBe('/end')
})

test('ignores inherited entries but follows own prototype-like keys', () => {
  const inherited = Object.create({ '/old': '/wrong' }) as Record<string, string>
  inherited['/start'] = '/old'
  expect(resolveRedirect('/start', inherited)).toBe('/old')
  expect(resolveRedirect('toString', {})).toBe('toString')
  const own = Object.create(null) as Record<string, string>
  own['__proto__'] = 'constructor'
  own['constructor'] = '/end'
  expect(resolveRedirect('__proto__', Object.freeze(own))).toBe('/end')
})
`,
    },
  },
]
