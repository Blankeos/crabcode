import type { GradedFixture } from '../types.ts'

// Explicit gates decide settlement; turn() drains ready callbacks without sleeping.
const helpers = `function gate<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
function turn(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve))
}
`

const visible = `import { expect, test } from 'bun:test'
import { createAsyncCache } from '../src/index.ts'
${helpers}
test('coalesces pending reads and caches undefined until the exact TTL boundary', async () => {
  let now = 0, calls = 0
  const hold = gate<undefined>()
  const cache = createAsyncCache(() => { calls++; return hold.promise }, { capacity: 2, ttlMs: 10, now: () => now })
  const first = cache.get('key'), second = cache.get('key')
  await turn()
  expect(calls).toBe(1)
  now = 100
  hold.resolve(undefined)
  expect(await first).toBeUndefined()
  expect(await second).toBeUndefined()
  now = 109
  expect(await cache.get('key')).toBeUndefined()
  expect(calls).toBe(1)
  now = 110
  expect(await cache.get('key')).toBeUndefined()
  expect(calls).toBe(2)
})

test('fulfilled hits refresh LRU and eviction reloads only the least recent key', async () => {
  const calls: string[] = []
  const cache = createAsyncCache((key) => { calls.push(key); return key }, { capacity: 2, ttlMs: 100, now: () => 0 })
  await cache.get('a'); await cache.get('b'); await cache.get('a'); await cache.get('c')
  await cache.get('a'); await cache.get('b')
  expect(calls).toEqual(['a', 'b', 'c', 'b'])
})

test('invalidation detaches a pending load without cancelling its callers', async () => {
  const old = gate<string>(), fresh = gate<string>()
  let calls = 0
  const cache = createAsyncCache(() => ++calls === 1 ? old.promise : fresh.promise,
    { capacity: 1, ttlMs: 10, now: () => 0 })
  const first = cache.get('x')
  await turn()
  cache.invalidate('x')
  const second = cache.get('x')
  await turn()
  fresh.resolve('new'); expect(await second).toBe('new')
  old.resolve('old'); expect(await first).toBe('old')
  expect(await cache.get('x')).toBe('new')
  expect(calls).toBe(2)
})
`

const behavior = `import { expect, test } from 'bun:test'
import { createAsyncCache } from '../src/index.ts'
${helpers}
test('validates only capacity and TTL at construction with RangeError', () => {
  let calls = 0
  const loader = () => { calls++; return 1 }
  for (const capacity of [0, -1, 1.5, NaN, Infinity, -Infinity]) {
    expect(() => createAsyncCache(loader, { capacity, ttlMs: 1, now: () => 0 })).toThrow(RangeError)
  }
  for (const ttlMs of [-1, NaN, Infinity, -Infinity]) {
    expect(() => createAsyncCache(loader, { capacity: 1, ttlMs, now: () => 0 })).toThrow(RangeError)
  }
  expect(calls).toBe(0)
})

test('zero TTL retains single-flight while pending but never yields a fulfilled hit', async () => {
  const hold = gate<number>()
  let calls = 0
  const cache = createAsyncCache(() => { calls++; return hold.promise }, { capacity: 1, ttlMs: 0, now: () => 4 })
  const a = cache.get('a'), b = cache.get('a')
  await turn(); expect(calls).toBe(1)
  hold.resolve(0)
  expect(await a).toBe(0); expect(await b).toBe(0)
  expect(await cache.get('a')).toBe(0)
  expect(calls).toBe(2)
})

test('hits do not extend TTL, fractional TTL works, and values/keys are unchanged', async () => {
  let now = 0
  const calls: string[] = []
  const object = { value: 1 }
  const values = new Map<string, unknown>([['', undefined], ['__proto__', object], ['constructor', false], [' ', null], ['zero', 0]])
  const cache = createAsyncCache((key) => { calls.push(key); return values.get(key) },
    { capacity: 5, ttlMs: 2.5, now: () => now })
  for (const [key, value] of values) expect(await cache.get(key)).toBe(value)
  now = 2
  for (const [key, value] of values) expect(await cache.get(key)).toBe(value)
  expect(calls).toHaveLength(5)
  now = 2.5
  expect(await cache.get('__proto__')).toBe(object)
  expect(calls).toHaveLength(6)
})

test('sync throws become Promise rejections and asynchronous failures are shared, not cached', async () => {
  for (const synchronous of [true, false]) {
    const reason = { failure: synchronous }
    const hold = gate<number>()
    let calls = 0
    const cache = createAsyncCache(() => {
      calls++
      if (calls > 1) return 7
      if (synchronous) throw reason
      return hold.promise
    }, { capacity: 1, ttlMs: 20, now: () => 0 })
    let first!: Promise<number>
    expect(() => { first = cache.get('bad') }).not.toThrow()
    expect(first).toBeInstanceOf(Promise)
    const outcome = first.then(() => 'unexpected', (error) => error)
    // A synchronous failure may already have settled before a second get; only
    // the gated asynchronous failure is required to deduplicate a second caller.
    const second = synchronous ? outcome : cache.get('bad').then(() => 'unexpected', (error) => error)
    await turn(); expect(calls).toBe(1)
    if (!synchronous) hold.reject(reason)
    expect(await outcome).toBe(reason); expect(await second).toBe(reason)
    expect(await cache.get('bad')).toBe(7)
    expect(await cache.get('bad')).toBe(7)
    expect(calls).toBe(2)
  }
})

test('pending entries do not consume capacity or gain recency; resolution inserts as MRU', async () => {
  const holds = new Map([['p', gate<string>()], ['q', gate<string>()]])
  const calls: string[] = []
  const cache = createAsyncCache((key) => { calls.push(key); return holds.get(key)?.promise ?? key },
    { capacity: 2, ttlMs: 100, now: () => 0 })
  await cache.get('a'); await cache.get('b')
  const p = cache.get('p'), q = cache.get('q')
  await turn()
  await cache.get('a'); await cache.get('b')
  expect(calls).toEqual(['a', 'b', 'p', 'q'])
  const p2 = cache.get('p')
  holds.get('q')!.resolve('q'); expect(await q).toBe('q')
  await cache.get('b')
  holds.get('p')!.resolve('p'); expect(await p).toBe('p'); expect(await p2).toBe('p')
  await cache.get('b'); await cache.get('p')
  expect(calls).toEqual(['a', 'b', 'p', 'q'])
  await cache.get('q')
  expect(calls).toEqual(['a', 'b', 'p', 'q', 'q'])
})

test('expired entries are discarded before eviction, even when expiry occurs during loading', async () => {
  let now = 0
  const hold = gate<string>()
  const calls: string[] = []
  const cache = createAsyncCache((key) => { calls.push(key); return key === 'new' ? hold.promise : key },
    { capacity: 2, ttlMs: 10, now: () => now })
  await cache.get('old')
  now = 5; await cache.get('live')
  now = 9; await cache.get('old')
  const pending = cache.get('new')
  await turn()
  now = 10; hold.resolve('new'); await pending; await cache.get('live')
  expect(calls).toEqual(['old', 'live', 'new'])
  await cache.get('old')
  expect(calls).toEqual(['old', 'live', 'new', 'old'])
})

test('invalidate affects only its exact key and clear removes fulfilled entries', async () => {
  const calls: string[] = []
  const cache = createAsyncCache((key) => { calls.push(key); return key },
    { capacity: 3, ttlMs: 10, now: () => 0 })
  await cache.get('a'); await cache.get('b')
  expect(cache.invalidate('missing')).toBeUndefined()
  expect(cache.invalidate('a')).toBeUndefined()
  await cache.get('b'); await cache.get('a')
  expect(calls).toEqual(['a', 'b', 'a'])
  expect(cache.clear()).toBeUndefined()
  cache.clear()
  await cache.get('a'); await cache.get('b')
  expect(calls).toEqual(['a', 'b', 'a', 'a', 'b'])
})
`

const races = `import { expect, test } from 'bun:test'
import { createAsyncCache } from '../src/index.ts'
${helpers}
for (const operation of ['invalidate', 'clear'] as const) {
  for (const rejects of [false, true]) {
    for (const oldFirst of [false, true]) {
      test(operation + ': stale ' + (rejects ? 'rejection' : 'fulfillment') + ', oldFirst=' + oldFirst, async () => {
        const old = gate<string>(), fresh = gate<string>()
        const reason = { stale: true }
        let calls = 0
        const cache = createAsyncCache(() => ++calls === 1 ? old.promise : fresh.promise,
          { capacity: 1, ttlMs: 10, now: () => 0 })
        const a = cache.get('key'), b = cache.get('key')
        const observed = Promise.all([a, b].map((p) => p.then((value) => value, (error) => error)))
        await turn(); expect(calls).toBe(1)
        if (operation === 'clear') cache.clear()
        else cache.invalidate('key')
        const next = cache.get('key')
        await turn(); expect(calls).toBe(2)
        if (oldFirst) {
          if (rejects) old.reject(reason); else old.resolve('old')
          expect(await observed).toEqual([rejects ? reason : 'old', rejects ? reason : 'old'])
          const joined = cache.get('key')
          await turn(); expect(calls).toBe(2)
          fresh.resolve('fresh'); expect(await joined).toBe('fresh')
        } else {
          fresh.resolve('fresh'); expect(await next).toBe('fresh')
          if (rejects) old.reject(reason); else old.resolve('old')
          expect(await observed).toEqual([rejects ? reason : 'old', rejects ? reason : 'old'])
        }
        expect(await next).toBe('fresh')
        expect(await cache.get('key')).toBe('fresh')
        expect(calls).toBe(2)
      })
    }
  }
}

test('clear detaches all keys and stale completion cannot evict unrelated fresh entries', async () => {
  const a = gate<string>(), b = gate<string>()
  const calls: string[] = []
  const cache = createAsyncCache((key) => {
    calls.push(key)
    return key === 'a' ? a.promise : key === 'b' ? b.promise : key
  }, { capacity: 1, ttlMs: 10, now: () => 0 })
  const pa = cache.get('a'), pb = cache.get('b')
  await turn(); cache.clear()
  await cache.get('c')
  a.resolve('old-a'); b.resolve('old-b')
  expect(await pa).toBe('old-a'); expect(await pb).toBe('old-b')
  expect(await cache.get('c')).toBe('c')
  expect(calls).toEqual(['a', 'b', 'c'])
  expect(await cache.get('a')).toBe('old-a')
  expect(calls).toEqual(['a', 'b', 'c', 'a'])
})
`

const task: GradedFixture = {
  id: 'async-cache',
  title: 'Repair a single-flight async cache with TTL, LRU and invalidation',
  difficulty: 'hard',
  prompt: `Repair createAsyncCache<T>(loader, options) exported from src/index.ts using the
supplied types, cache and LRU modules. Keep the public API; helpers in src/ are allowed.
get(key) always returns a Promise. Keys are exact strings (including empty strings).
Load on a miss; overlapping reads of the same pending key share one loader invocation
and its unchanged value or rejection reason. Promise identity is not required. Invoke
the loader synchronously or in a microtask, not on a timer. Sync throws reject get,
never escape it. Failures are removed before callers observe rejection so the next
read retries. Values including undefined are valid cached results.

TTL uses ONLY options.now(): a nonthrowing, finite, nondecreasing injected clock.
Start TTL when a load fulfills, not when requested. A fulfilled entry is fresh iff
now() < resolvedAt + ttlMs; equality is expired. Hits do not extend TTL. TTL zero
still deduplicates pending loads but never serves a fulfilled hit.

Capacity counts only fulfilled, unexpired entries; pending loads are unbounded and
never evicted. Discard expired entries before capacity eviction. Fulfillment inserts
as most recently used; fulfilled hits refresh LRU, pending reads do not. Evict the
least recently used fulfilled entry when necessary. Different keys load independently.

invalidate(key): void detaches just that key; clear(): void detaches all entries.
Neither cancels loads: existing callers still receive their original result/error.
Subsequent reads start a new generation. Detached completions (success OR failure)
must never restore entries, delete newer pending/fulfilled entries or trigger eviction.

At construction, throw RangeError for capacity that is not a finite positive integer
or ttlMs that is not finite and nonnegative (fractional TTL is valid). Other inputs
conform to types; no other validation, cancellation, timers or retries are needed.
Work only in this workspace, no dependencies/installs/network. Keep visible tests and
run bun test. The clean-room grader copies only src/ and runs immutable visible/hidden
tests; edits to tests, scripts, README or configuration cannot change grading.`,
  files: {
    'package.json': '{"type":"module","scripts":{"test":"bun test"}}\n',
    'README.md': `# Async cache repair

Run \`bun test\`; no install needed. src/types.ts defines the API, src/cache.ts
coordinates loading, src/lru.ts stores fulfilled entries, and src/index.ts exports
createAsyncCache. The starter compiles but omits single-flight, TTL and race handling.
Follow the task prompt; tests use an injected clock and explicit completion gates.
`,
    'src/types.ts': `export type CacheOptions = {
  readonly capacity: number
  readonly ttlMs: number
  readonly now: () => number
}
export type Loader<T> = (key: string) => T | Promise<T>
export type AsyncCache<T> = {
  get(key: string): Promise<T>
  invalidate(key: string): void
  clear(): void
}
`,
    'src/lru.ts': `export type Fulfilled<T> = { value: T, expiresAt: number }

export class Lru<T> {
  private entries = new Map<string, Fulfilled<T>>()
  constructor(private capacity: number) {}
  read(key: string, now: number): Fulfilled<T> | undefined {
    // TODO: expiry and recency.
    return this.entries.get(key)
  }
  write(key: string, entry: Fulfilled<T>, now: number): void {
    // TODO: purge expiry and evict least recent fulfilled entries only.
    this.entries.set(key, entry)
  }
  delete(key: string): void { this.entries.delete(key) }
  clear(): void { this.entries.clear() }
}
`,
    'src/cache.ts': `import { Lru } from './lru.ts'
import type { AsyncCache, CacheOptions, Loader } from './types.ts'

export function createAsyncCache<T>(loader: Loader<T>, options: CacheOptions): AsyncCache<T> {
  const fulfilled = new Lru<T>(options.capacity)
  return {
    async get(key) {
      const hit = fulfilled.read(key, options.now())
      if (hit) return hit.value
      // TODO: single-flight, resolution-time TTL, and detached generations.
      const value = await loader(key)
      fulfilled.write(key, { value, expiresAt: options.now() + options.ttlMs }, options.now())
      return value
    },
    invalidate(key) { fulfilled.delete(key) },
    clear() { fulfilled.clear() },
  }
}
`,
    'src/index.ts': `export { createAsyncCache } from './cache.ts'
export type { AsyncCache, CacheOptions, Loader } from './types.ts'
`,
    'tests/cache.test.ts': visible,
  },
  checks: {
    'tests/cache.test.ts': visible,
    'tests/cache.behavior.hidden.test.ts': behavior,
    'tests/cache.races.hidden.test.ts': races,
  },
}

// Registration/opt-in metadata belongs to the parent task adapter, not this fixture.
export const asyncCacheTasks: GradedFixture[] = [task]
