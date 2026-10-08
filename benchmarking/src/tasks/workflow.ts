import type { GradedFixture } from '../types.ts'

const packageJson = JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n'

// Gates control completion explicitly; an event-loop turn only drains ready callbacks, not a timed sleep.
const asyncHelpers = `function gate() {
  let open!: () => void
  const promise = new Promise<void>((resolve) => { open = resolve })
  return { promise, open }
}

function turn(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve))
}
`

const workflowVisible = `import { expect, test } from 'bun:test'
import { runWorkflow, type Job } from '../src/index.ts'

${asyncHelpers}
test('runs a dependency first but reports in original input order', async () => {
  const calls: string[] = []
  const jobs: Job[] = [
    { id: 'double', deps: ['base'], run: (values) => {
      calls.push('double')
      expect(values).toEqual({ base: 3 })
      return Number(values.base) * 2
    } },
    { id: 'base', run: (values) => {
      calls.push('base')
      expect(values).toEqual({})
      return 3
    } },
  ]
  expect(await runWorkflow(jobs, 2)).toEqual([
    { id: 'double', status: 'succeeded', value: 6 },
    { id: 'base', status: 'succeeded', value: 3 },
  ])
  expect(calls).toEqual(['base', 'double'])
})

test('blocks a failed dependency without stopping an independent job', async () => {
  const error = new Error('broken root')
  let blockedCalls = 0
  let independentCalls = 0
  const results = await runWorkflow([
    { id: 'bad', run: () => { throw error } },
    { id: 'child', deps: ['bad'], run: () => { blockedCalls++; return 9 } },
    { id: 'other', run: async () => { independentCalls++; return 7 } },
  ], 2)
  expect(results).toEqual([
    { id: 'bad', status: 'failed', error },
    { id: 'child', status: 'blocked' },
    { id: 'other', status: 'succeeded', value: 7 },
  ])
  expect(results[0].status === 'failed' && results[0].error).toBe(error)
  expect(blockedCalls).toBe(0)
  expect(independentCalls).toBe(1)
})

test('concurrency one holds the next independent job until a slot is free', async () => {
  const hold = gate()
  const started: string[] = []
  const jobs = ['one', 'two'].map((id): Job => ({ id, run: async () => {
    started.push(id)
    await hold.promise
    return id
  } }))
  const pending = runWorkflow(jobs, 1)
  try {
    await turn()
    expect(started).toHaveLength(1)
  } finally {
    hold.open()
    await pending
  }
  expect(new Set(started).size).toBe(2)
  expect(await pending).toEqual(jobs.map(({ id }) => ({ id, status: 'succeeded', value: id })))
})
`

const workflowGraphHidden = `import { describe, expect, test } from 'bun:test'
import { runWorkflow, type Job } from '../src/index.ts'

describe('graph validation and dependency values', () => {
  test('handles empty workflows and synchronous or promised falsy values', async () => {
    expect(await runWorkflow([], 1)).toEqual([])
    const jobs: Job[] = [
      { id: 'zero', run: () => 0 },
      { id: 'false', deps: [], run: async () => false },
      { id: 'undefined', run: () => undefined },
      { id: 'null', run: async () => null },
    ]
    expect(await runWorkflow(jobs, 8)).toEqual([
      { id: 'zero', status: 'succeeded', value: 0 },
      { id: 'false', status: 'succeeded', value: false },
      { id: 'undefined', status: 'succeeded', value: undefined },
      { id: 'null', status: 'succeeded', value: null },
    ])
  })

  test('executes a reversed transitive diamond once with only direct dependency values', async () => {
    const root = { token: 4 }
    const joined = { answer: 8 }
    const calls: Record<string, number> = {}
    function job(id: string, deps: string[], run: Job['run']): Job {
      return Object.freeze({ id, deps: Object.freeze(deps), run: (values) => {
        calls[id] = (calls[id] ?? 0) + 1
        return run(values)
      } })
    }
    const jobs = Object.freeze([
      job('tail', ['join'], (values) => {
        expect(Object.keys(values)).toEqual(['join'])
        expect(values.join).toBe(joined)
        return ''
      }),
      job('join', ['left', 'right'], (values) => {
        expect(Object.keys(values).sort()).toEqual(['left', 'right'])
        expect(values.left).toBe(0)
        expect(values.right).toBe(undefined)
        return joined
      }),
      job('right', ['root'], async (values) => {
        expect(Object.keys(values)).toEqual(['root'])
        expect(values.root).toBe(root)
        return undefined
      }),
      job('left', ['root'], (values) => {
        expect(Object.keys(values)).toEqual(['root'])
        expect(values.root).toBe(root)
        return 0
      }),
      job('root', [], (values) => { expect(Object.keys(values)).toEqual([]); return root }),
    ])
    const results = await runWorkflow(jobs, 3)
    expect(results).toEqual([
      { id: 'tail', status: 'succeeded', value: '' },
      { id: 'join', status: 'succeeded', value: joined },
      { id: 'right', status: 'succeeded', value: undefined },
      { id: 'left', status: 'succeeded', value: 0 },
      { id: 'root', status: 'succeeded', value: root },
    ])
    expect(calls).toEqual({ root: 1, left: 1, right: 1, join: 1, tail: 1 })
    expect(results[1].status === 'succeeded' && results[1].value).toBe(joined)
    expect(results[4].status === 'succeeded' && results[4].value).toBe(root)
  })

  test('treats all nonempty IDs as exact strings, including object-key-like names', async () => {
    const value = { ok: true }
    expect(await runWorkflow([
      { id: 'consumer', deps: ['__proto__', 'constructor', ' '], run: (values) => {
        expect(Object.keys(values).sort()).toEqual([' ', '__proto__', 'constructor'])
        expect(Object.hasOwn(values, '__proto__')).toBe(true)
        expect(values.__proto__).toBe(value)
        expect(values.constructor).toBe(0)
        expect(values[' ']).toBe(false)
        return 'ok'
      } },
      { id: '__proto__', run: () => value },
      { id: 'constructor', run: () => 0 },
      { id: ' ', run: () => false },
    ], 2)).toEqual([
      { id: 'consumer', status: 'succeeded', value: 'ok' },
      { id: '__proto__', status: 'succeeded', value },
      { id: 'constructor', status: 'succeeded', value: 0 },
      { id: ' ', status: 'succeeded', value: false },
    ])
  })

  test('rejects invalid graphs before any run callback, including disconnected cycles', async () => {
    let effects = 0
    const run = () => { effects++; return 1 }
    const invalid: Job[][] = [
      [{ id: '', run }],
      [{ id: 'same', run }, { id: 'same', run }],
      [{ id: 'a', deps: ['missing'], run }],
      [{ id: 'a', deps: ['a'], run }],
      [{ id: 'a', run }, { id: 'b', deps: ['a', 'a'], run }],
      [{ id: 'a', deps: ['b'], run }, { id: 'b', deps: ['a'], run }],
      [{ id: 'tail', deps: ['a'], run }, { id: 'a', deps: ['b'], run },
       { id: 'b', deps: ['c'], run }, { id: 'c', deps: ['a'], run }],
    ]
    for (const jobs of invalid) {
      await expect(runWorkflow([{ id: 'safe', run }, ...jobs], 2)).rejects.toThrow(Error)
      expect(effects).toBe(0)
    }
  })

  test('rejects nonpositive, fractional and nonfinite concurrency even for empty input', async () => {
    let effects = 0
    for (const concurrency of [0, -1, 1.5, NaN, Infinity, -Infinity]) {
      await expect(runWorkflow([], concurrency)).rejects.toThrow(RangeError)
      await expect(runWorkflow([{ id: 'a', run: () => { effects++; return 1 } }], concurrency))
        .rejects.toThrow(RangeError)
      expect(effects).toBe(0)
    }
  })
})
`

const workflowSchedulerHidden = `import { describe, expect, test } from 'bun:test'
import { runWorkflow, type Job } from '../src/index.ts'

${asyncHelpers}
describe('asynchronous scheduling and failure isolation', () => {
  test('refills a freed slot with a ready dependent without waiting for a slow sibling', async () => {
    const slow = gate()
    const fast = gate()
    const started: string[] = []
    const finished: string[] = []
    let settled = false
    const pending = runWorkflow([
      { id: 'slow', run: async () => {
        started.push('slow'); await slow.promise; finished.push('slow'); return 10
      } },
      { id: 'child', deps: ['fast'], run: (values) => {
        started.push('child'); expect(values).toEqual({ fast: 2 }); finished.push('child'); return 3
      } },
      { id: 'fast', run: async () => {
        started.push('fast'); await fast.promise; finished.push('fast'); return 2
      } },
    ], 2)
    void pending.then(() => { settled = true })
    try {
      await turn()
      expect([...started].sort()).toEqual(['fast', 'slow'])
      fast.open()
      await turn()
      expect(finished).toEqual(['fast', 'child'])
      expect(settled).toBe(false)
    } finally {
      fast.open()
      slow.open()
      await pending
    }
    expect(finished).toEqual(['fast', 'child', 'slow'])
    expect(await pending).toEqual([
      { id: 'slow', status: 'succeeded', value: 10 },
      { id: 'child', status: 'succeeded', value: 3 },
      { id: 'fast', status: 'succeeded', value: 2 },
    ])
  })

  test('fills and refills available slots but never exceeds the concurrency cap', async () => {
    for (const concurrency of [1, 2, 3, 8]) {
      const holds = Array.from({ length: 6 }, () => gate())
      const started: number[] = []
      let active = 0
      let peak = 0
      const jobs = holds.map((hold, index): Job => ({ id: 'job-' + index, run: async () => {
        started.push(index)
        active++
        peak = Math.max(peak, active)
        await hold.promise
        active--
        return index
      } }))
      const pending = runWorkflow(jobs, concurrency)
      try {
        await turn()
        expect(started).toHaveLength(Math.min(concurrency, jobs.length))
        expect(active).toBe(Math.min(concurrency, jobs.length))
        holds[started[0]].open()
        await turn()
        expect(started).toHaveLength(Math.min(concurrency + 1, jobs.length))
        expect(active).toBe(Math.min(concurrency, jobs.length - 1))
        expect(peak).toBeLessThanOrEqual(concurrency)
      } finally {
        for (const hold of holds) hold.open()
        await pending
      }
      expect(active).toBe(0)
      expect(peak).toBe(Math.min(concurrency, jobs.length))
      expect(new Set(started).size).toBe(jobs.length)
      expect(await pending).toEqual(jobs.map(({ id }, value) => ({ id, status: 'succeeded', value })))
    }
  })

  test('blocks the full failed descendant closure and still executes independent branches', async () => {
    const reason = { code: 'async failure' }
    const calls: Record<string, number> = {}
    function job(id: string, deps: string[], run: Job['run']): Job {
      return { id, deps, run: (values) => {
        calls[id] = (calls[id] ?? 0) + 1
        return run(values)
      } }
    }
    const results = await runWorkflow([
      job('grandchild', ['child'], () => 99),
      job('child', ['bad', 'good'], () => 99),
      job('bad', [], async () => { throw reason }),
      job('sync-failure', [], () => { throw 'original reason' }),
      job('good-child', ['good'], (values) => { expect(values).toEqual({ good: 7 }); return 8 }),
      job('good', [], async () => 7),
      job('another', [], () => 0),
    ], 2)
    expect(results).toEqual([
      { id: 'grandchild', status: 'blocked' },
      { id: 'child', status: 'blocked' },
      { id: 'bad', status: 'failed', error: reason },
      { id: 'sync-failure', status: 'failed', error: 'original reason' },
      { id: 'good-child', status: 'succeeded', value: 8 },
      { id: 'good', status: 'succeeded', value: 7 },
      { id: 'another', status: 'succeeded', value: 0 },
    ])
    expect(results[2].status === 'failed' && results[2].error).toBe(reason)
    expect(calls).toEqual({ bad: 1, 'sync-failure': 1, good: 1, 'good-child': 1, another: 1 })
  })
})
`

const workflowTask: GradedFixture & { difficulty: 'medium' } = {
  id: 'workflow-runner',
  title: 'Repair a dependency-aware asynchronous workflow runner',
  difficulty: 'medium',
  prompt: `Repair runWorkflow(jobs, concurrency) exported from src/index.ts, using the types,
plan and runner modules. Jobs have id, optional readonly deps (default []), and run(values)
returning a value or Promise. Keep the public API/types; helpers inside src/ are allowed.

Validate the entire graph BEFORE calling any job: IDs must be unique nonempty strings;
reject missing dependencies, self-dependencies, duplicate dependency IDs and cycles,
including disconnected cycles. IDs are exact strings (whitespace and object-key-like names
are valid). Invalid graphs reject the workflow Promise with Error; messages are unspecified.
Concurrency must be a finite positive integer: reject the Promise with RangeError even for empty jobs.
Other inputs conform to the supplied types; no additional runtime validation is required.

Execute each eligible job exactly once, only after ALL its dependencies succeed. Pass a
record containing ONLY direct dependency IDs and their unchanged values, including falsy
or undefined values; roots receive an empty record. Fill available slots with ready jobs,
never exceed concurrency, and refill as individual jobs settle rather than waiting for a
whole batch. No ordering between simultaneously ready jobs is required.

A synchronous throw or rejected Promise fails that job. Preserve its original error/reason.
Every descendant of a failed job becomes blocked WITHOUT execution. Independent branches
must continue, including jobs waiting for slots and their dependents; job failures must not
reject the workflow Promise. Return one result per job in ORIGINAL input order, regardless
of completion order: { id, status: 'succeeded', value }, { id, status: 'failed', error }, or
{ id, status: 'blocked' }. Empty input returns []. Do not mutate jobs or dependency arrays.

Work only in your own provided workspace. No dependencies, installs, network, cancellation
or retries. Run bun test before finishing. Keep visible tests. The clean-room grader copies
only src/ source modules and runs immutable visible/hidden tests; candidate tests, package
scripts, README and configuration cannot affect grading.`,
  files: {
    'package.json': packageJson,
    'README.md': `# Workflow runner

Run \`bun test\` or \`bun run test\` from this workspace. No install is needed.

- src/index.ts: public runWorkflow export and Job/JobResult types
- src/types.ts: dependency inputs and discriminated result shapes
- src/plan.ts: graph validation and planning
- src/runner.ts: asynchronous scheduling and ordered results
- tests/workflow.test.ts: visible dependency, failure and concurrency regressions

The current implementation compiles but executes sequentially in input order and skips
validation. Repair it according to the task prompt; no external services are involved.
`,
    'src/types.ts': `export type Job = {
  readonly id: string
  readonly deps?: readonly string[]
  readonly run: (values: Readonly<Record<string, unknown>>) => unknown | Promise<unknown>
}

export type JobResult =
  | { id: string, status: 'succeeded', value: unknown }
  | { id: string, status: 'failed', error: unknown }
  | { id: string, status: 'blocked' }
`,
    'src/plan.ts': `import type { Job } from './types.ts'

export type PlannedJob = { job: Job, deps: readonly string[] }

export function planWorkflow(jobs: readonly Job[], concurrency: number): Map<string, PlannedJob> {
  // TODO: validate concurrency, IDs, dependency edges and cycles before running anything.
  return new Map(jobs.map((job) => [job.id, { job, deps: job.deps ?? [] }]))
}
`,
    'src/runner.ts': `import { planWorkflow } from './plan.ts'
import type { Job, JobResult } from './types.ts'

export async function runWorkflow(jobs: readonly Job[], concurrency: number): Promise<JobResult[]> {
  const plan = planWorkflow(jobs, concurrency)
  const results: JobResult[] = []
  const values: Record<string, unknown> = {}
  // TODO: schedule by dependencies, isolate failures and enforce the concurrency cap.
  for (const { job } of plan.values()) {
    try {
      const value = await job.run(values)
      values[job.id] = value
      results.push({ id: job.id, status: 'succeeded', value })
    } catch (error) {
      results.push({ id: job.id, status: 'failed', error })
    }
  }
  return results
}
`,
    'src/index.ts': `export { runWorkflow } from './runner.ts'
export type { Job, JobResult } from './types.ts'
`,
    'tests/workflow.test.ts': workflowVisible,
  },
  checks: {
    'tests/workflow.test.ts': workflowVisible,
    'tests/workflow.graph.hidden.test.ts': workflowGraphHidden,
    'tests/workflow.scheduler.hidden.test.ts': workflowSchedulerHidden,
  },
}

// Registration is intentionally separate so the default four smoke tasks stay unchanged.
export const workflowTasks: GradedFixture[] = [workflowTask]
