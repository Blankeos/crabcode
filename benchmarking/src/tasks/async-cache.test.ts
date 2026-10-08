import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gradeTask } from '../checks.ts'
import type { Grade } from '../types.ts'
import { writeFiles } from '../workspace.ts'
import { asyncCacheTasks } from './async-cache.ts'

// This implementation is harness-only: never included in the candidate workspace.
const referenceSources = {
  'src/lru.ts': `export type Fulfilled<T> = { value: T, expiresAt: number }

export class Lru<T> {
  private entries = new Map<string, Fulfilled<T>>()
  constructor(private capacity: number) {}
  private purge(now: number): void {
    for (const [key, entry] of this.entries) {
      if (now >= entry.expiresAt) this.entries.delete(key)
    }
  }
  read(key: string, now: number): Fulfilled<T> | undefined {
    this.purge(now)
    const entry = this.entries.get(key)
    if (entry) {
      this.entries.delete(key)
      this.entries.set(key, entry)
    }
    return entry
  }
  write(key: string, entry: Fulfilled<T>, now: number): void {
    this.purge(now)
    this.entries.delete(key)
    if (now >= entry.expiresAt) return
    this.entries.set(key, entry)
    while (this.entries.size > this.capacity) {
      this.entries.delete(this.entries.keys().next().value!)
    }
  }
  delete(key: string): void { this.entries.delete(key) }
  clear(): void { this.entries.clear() }
}
`,
  'src/cache.ts': `import { Lru } from './lru.ts'
import type { AsyncCache, CacheOptions, Loader } from './types.ts'

export function createAsyncCache<T>(loader: Loader<T>, options: CacheOptions): AsyncCache<T> {
  const { capacity, ttlMs, now } = options
  if (!Number.isInteger(capacity) || capacity <= 0 || !Number.isFinite(ttlMs) || ttlMs < 0) {
    throw new RangeError('invalid cache options')
  }
  const fulfilled = new Lru<T>(capacity)
  const pending = new Map<string, Promise<T>>()
  return {
    get(key) {
      const hit = fulfilled.read(key, now())
      if (hit) return Promise.resolve(hit.value)
      const existing = pending.get(key)
      if (existing) return existing
      // Install the generation before invoking user code. Deferral also normalizes sync throws.
      const promise = Promise.resolve().then(() => loader(key)).then((value) => {
        if (pending.get(key) === promise) {
          pending.delete(key)
          const resolvedAt = now()
          fulfilled.write(key, { value, expiresAt: resolvedAt + ttlMs }, resolvedAt)
        }
        return value
      }, (reason) => {
        if (pending.get(key) === promise) pending.delete(key)
        throw reason
      })
      pending.set(key, promise)
      return promise
    },
    invalidate(key) { pending.delete(key); fulfilled.delete(key) },
    clear() { pending.clear(); fulfilled.clear() },
  }
}
`,
}

const task = asyncCacheTasks[0]!

function grade(sources: Record<string, string> = {}): Grade {
  const root = mkdtempSync(join(tmpdir(), 'crabcode-async-cache-test-'))
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

test('async-cache exports one hard fixture, exposing only immutable visible tests', () => {
  expect(asyncCacheTasks).toHaveLength(1)
  expect(task.id).toBe('async-cache')
  expect(task.difficulty).toBe('hard')
  expect(Object.keys(task.files).filter((path) => path.endsWith('.test.ts'))).toEqual(['tests/cache.test.ts'])
  expect(task.files['tests/cache.test.ts']).toBe(task.checks['tests/cache.test.ts'])
})

test('async-cache starter fails assertions and reference passes all actual grader suites', () => {
  const baseline = grade()
  expect(baseline.passed).toBe(false)
  for (const { name } of baseline.checks) expectSuite(baseline, name, false)
  const reference = grade(referenceSources)
  expect(reference.passed, JSON.stringify(reference.checks)).toBe(true)
  for (const { name } of reference.checks) expectSuite(reference, name, true)
})

test('grader ignores candidate edits to visible/hidden tests and package scripts', () => {
  const result = grade({
    'tests/cache.test.ts': "import { test } from 'bun:test'; test('fake pass', () => {})",
    'tests/cache.behavior.hidden.test.ts': '',
    'tests/cache.races.hidden.test.ts': '',
    'package.json': '{"scripts":{"test":"exit 0"}}',
  })
  expect(result.passed).toBe(false)
  for (const { name } of result.checks) expectSuite(result, name, false)
})

test('race grader catches stale successes and stale rejection cleanup separately', () => {
  for (const guard of [
    'if (pending.get(key) === promise) {',
    'if (pending.get(key) === promise) pending.delete(key)',
  ]) {
    const cache = replaceOnce(
      referenceSources['src/cache.ts'],
      guard,
      guard.endsWith('{') ? 'if (true) {' : 'pending.delete(key)',
    )
    const result = grade({ ...referenceSources, 'src/cache.ts': cache })
    expect(result.passed).toBe(false)
    expectSuite(result, 'tests/cache.behavior.hidden.test.ts', true)
    expectSuite(result, 'tests/cache.races.hidden.test.ts', false)
  }
})

test('behavior grader catches sliding TTL, an inclusive TTL boundary and missing hit recency', () => {
  const mutants = [
    replaceOnce(
      referenceSources['src/cache.ts'],
      'if (hit) return Promise.resolve(hit.value)',
      'if (hit) { hit.expiresAt = now() + ttlMs; return Promise.resolve(hit.value) }',
    ),
    replaceOnce(
      referenceSources['src/lru.ts'],
      'if (now >= entry.expiresAt) this.entries.delete(key)',
      'if (now > entry.expiresAt) this.entries.delete(key)',
    ),
    replaceOnce(
      referenceSources['src/lru.ts'],
      `    if (entry) {
      this.entries.delete(key)
      this.entries.set(key, entry)
    }`,
      '',
    ),
  ]
  for (const [index, source] of mutants.entries()) {
    const path = index === 0 ? 'src/cache.ts' : 'src/lru.ts'
    const result = grade({ ...referenceSources, [path]: source })
    expect(result.passed).toBe(false)
    expectSuite(result, 'tests/cache.behavior.hidden.test.ts', false)
  }
})
