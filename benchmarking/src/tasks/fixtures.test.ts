import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gradeTask } from '../checks.ts'
import type { Grade, GradedFixture } from '../types.ts'
import { writeFiles } from '../workspace.ts'
import { codingTasks } from './coding.ts'
import { workflowTasks } from './workflow.ts'

// Complete references stay in harness tests; unchanged types and entry points come from task.files.
const referenceSources: Record<string, Record<string, string>> = {
  'page-boundaries': {
    'src/page.ts': `export function page<T>(items: readonly T[], offset: number, limit: number): T[] {
  if (!Number.isInteger(offset) || offset < 0 || !Number.isInteger(limit) || limit < 0) {
    throw new RangeError('offset and limit must be nonnegative integers')
  }
  return items.slice(offset, offset + limit)
}
`,
  },
  'job-status': {
    'src/status.ts': `export type JobStatus = 'queued' | 'blocked' | 'running' | 'done'

export const STATUS_ORDER: readonly JobStatus[] = ['queued', 'blocked', 'running', 'done']

export function statusLabel(status: JobStatus): string {
  const labels: Record<JobStatus, string> = {
    queued: 'Queued', blocked: 'Blocked', running: 'Running', done: 'Done',
  }
  return labels[status]
}
`,
    'src/summary.ts': `import { statusLabel, type JobStatus } from './status.ts'

export type Job = { id: string, status: JobStatus }

export function summarizeJobs(jobs: readonly Job[]): { counts: Record<JobStatus, number>, lines: string[] } {
  const counts: Record<JobStatus, number> = { queued: 0, blocked: 0, running: 0, done: 0 }
  const lines: string[] = []
  for (const job of jobs) {
    counts[job.status] += 1
    lines.push(job.id + ': ' + statusLabel(job.status))
  }
  return { counts, lines }
}
`,
  },
  'billing-refactor': {
    'src/billing/blocks.ts': `export function billableBlocks(used: number, included: number, blockSize: number): number {
  return Math.ceil(Math.max(0, used - included) / blockSize)
}
`,
    'src/billing/costs.ts': `import { billableBlocks } from './blocks.ts'

export function storageCost(used: number, included: number, centsPerBlock: number): number {
  return billableBlocks(used, included, 5) * centsPerBlock
}

export function transferCost(used: number, included: number, centsPerBlock: number): number {
  return billableBlocks(used, included, 10) * centsPerBlock
}
`,
  },
  'noisy-redirects': {
    'packages/router/src/redirects.ts': `export function resolveRedirect(path: string, redirects: Readonly<Record<string, string>>): string {
  const seen = new Set<string>()
  let current = path
  while (Object.prototype.hasOwnProperty.call(redirects, current)) {
    if (seen.has(current)) return path
    seen.add(current)
    current = redirects[current]
  }
  return current
}
`,
  },
  'workflow-runner': {
    'src/plan.ts': `import type { Job } from './types.ts'

export type PlannedJob = { job: Job, deps: readonly string[] }

export function planWorkflow(jobs: readonly Job[], concurrency: number): Map<string, PlannedJob> {
  if (!Number.isInteger(concurrency) || concurrency <= 0) {
    throw new RangeError('concurrency must be a finite positive integer')
  }
  const plan = new Map<string, PlannedJob>()
  for (const job of jobs) {
    if (typeof job.id !== 'string' || job.id.length === 0 || plan.has(job.id)) {
      throw new Error('job IDs must be unique nonempty strings')
    }
    plan.set(job.id, { job, deps: [...(job.deps ?? [])] })
  }
  for (const [id, { deps }] of plan) {
    const seen = new Set<string>()
    for (const dep of deps) {
      if (!plan.has(dep) || dep === id || seen.has(dep)) {
        throw new Error('invalid dependency')
      }
      seen.add(dep)
    }
  }
  const visiting = new Set<string>()
  const visited = new Set<string>()
  function visit(id: string) {
    if (visiting.has(id)) throw new Error('dependency cycle')
    if (visited.has(id)) return
    visiting.add(id)
    for (const dep of plan.get(id)!.deps) visit(dep)
    visiting.delete(id)
    visited.add(id)
  }
  for (const id of plan.keys()) visit(id)
  return plan
}
`,
    'src/runner.ts': `import { planWorkflow } from './plan.ts'
import type { Job, JobResult } from './types.ts'

export async function runWorkflow(jobs: readonly Job[], concurrency: number): Promise<JobResult[]> {
  const plan = planWorkflow(jobs, concurrency)
  const pending = new Set(plan.keys())
  const results = new Map<string, JobResult>()
  let active = 0

  return new Promise((resolve) => {
    function pump() {
      // Rescan so blocked descendants propagate regardless of input order.
      let changed = true
      while (changed) {
        changed = false
        for (const id of pending) {
          const { job, deps } = plan.get(id)!
          const blocked = deps.some((dep) => {
            const status = results.get(dep)?.status
            return status === 'failed' || status === 'blocked'
          })
          if (blocked) {
            pending.delete(id)
            results.set(id, { id, status: 'blocked' })
            changed = true
            continue
          }
          if (active >= concurrency || !deps.every((dep) => results.get(dep)?.status === 'succeeded')) continue

          pending.delete(id)
          active++
          changed = true
          const values = Object.fromEntries(deps.map((dep) => {
            const result = results.get(dep) as Extract<JobResult, { status: 'succeeded' }>
            return [dep, result.value]
          }))
          // Deferring invocation catches synchronous throws as well as rejected promises.
          void Promise.resolve()
            .then(() => job.run(values))
            .then((value) => {
              results.set(id, { id, status: 'succeeded', value })
            }, (error) => {
              results.set(id, { id, status: 'failed', error })
            })
            .finally(() => {
              active--
              pump()
            })
        }
      }
      if (pending.size === 0 && active === 0) {
        resolve(jobs.map((job) => results.get(job.id)!))
      }
    }
    pump()
  })
}
`,
  },
}

function grade(task: GradedFixture, sources: Record<string, string> = {}): Grade {
  const root = mkdtempSync(join(tmpdir(), 'crabcode-fixture-test-'))
  try {
    writeFiles(root, { ...task.files, ...sources })
    const result = gradeTask(task, root)
    expect(result.checks.map(({ name }) => name)).toEqual(Object.keys(task.checks))
    for (const { output } of result.checks) {
      expect(output).toMatch(/[1-9]\d* (pass|fail)/)
      expect(output).not.toMatch(/Cannot find module|SyntaxError|error between tests|timed out/)
    }
    return result
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

function expectSuite(result: Grade, name: string, passed: boolean) {
  const check = result.checks.find((check) => check.name === name)!
  expect(check.passed, check.output).toBe(passed)
  expect(check.output).toMatch(passed ? /[1-9]\d* pass/ : /[1-9]\d* fail/)
  if (passed) expect(check.output).toContain('0 fail')
  else expect(check.output).toContain('expect(')
  return check.output
}

function replaceOnce(source: string, before: string, after: string): string {
  expect(source.split(before)).toHaveLength(2)
  return source.replace(before, after)
}

const fixtures = [...codingTasks, ...workflowTasks]

test('fixture IDs are unique and each has a reference', () => {
  expect(new Set(fixtures.map(({ id }) => id)).size).toBe(fixtures.length)
  expect(fixtures.map(({ id }) => id).sort()).toEqual(Object.keys(referenceSources).sort())
})

for (const task of fixtures) {
  test(`${task.id}: baseline fails regressions and reference passes every grader suite`, () => {
    const baseline = grade(task)
    expect(baseline.passed).toBe(false)
    for (const { name } of baseline.checks) {
      // The unrelated archive utility is already correct and must remain so.
      expectSuite(baseline, name, task.id === 'noisy-redirects' && name === 'tests/archive.test.ts')
    }
    const reference = grade(task, referenceSources[task.id])
    expect(reference.passed, JSON.stringify(reference.checks)).toBe(true)
    for (const { name } of reference.checks) expectSuite(reference, name, true)
  })
}

test('billing grader rejects an implemented but unused shared helper', () => {
  const task = codingTasks.find(({ id }) => id === 'billing-refactor')!
  const result = grade(task, { 'src/billing/blocks.ts': referenceSources[task.id]['src/billing/blocks.ts'] })
  expect(result.passed).toBe(false)
  expectSuite(result, 'tests/billing.test.ts', true)
  expect(expectSuite(result, 'tests/billing.hidden.test.ts', false)).toContain(
    '(fail) both existing cost functions delegate',
  )
})

const workflow = workflowTasks.find(({ id }) => id === 'workflow-runner')!
const workflowReference = referenceSources[workflow.id]

test('workflow grader rejects a scheduler without a concurrency cap', () => {
  const runner = replaceOnce(workflowReference['src/runner.ts'], 'active >= concurrency', 'false')
  const result = grade(workflow, { ...workflowReference, 'src/runner.ts': runner })
  expect(result.passed).toBe(false)
  expectSuite(result, 'tests/workflow.test.ts', false)
  expect(expectSuite(result, 'tests/workflow.scheduler.hidden.test.ts', false)).toContain(
    '(fail) asynchronous scheduling and failure isolation > fills and refills',
  )
})

test('workflow grader rejects blocking only immediate children of failed jobs', () => {
  let runner = replaceOnce(
    workflowReference['src/runner.ts'],
    "return status === 'failed' || status === 'blocked'",
    "return status === 'failed'",
  )
  runner = replaceOnce(runner, "results.get(dep)?.status === 'succeeded'", 'results.has(dep)')
  const result = grade(workflow, { ...workflowReference, 'src/runner.ts': runner })
  expect(result.passed).toBe(false)
  expectSuite(result, 'tests/workflow.test.ts', true)
  expect(expectSuite(result, 'tests/workflow.scheduler.hidden.test.ts', false)).toContain(
    '(fail) asynchronous scheduling and failure isolation > blocks the full failed',
  )
})
