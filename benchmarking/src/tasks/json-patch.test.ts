import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gradeTask } from '../checks.ts'
import type { Grade } from '../types.ts'
import { writeFiles } from '../workspace.ts'
import { jsonPatchTasks } from './json-patch.ts'

// Complete implementations live only in harness tests, never in the candidate workspace.
const reference: Record<string, string> = {
  'src/pointer.ts': `export function parsePointer(path: string): string[] {
  if (path === '') return []
  if (!path.startsWith('/')) throw new Error('invalid pointer')
  return path.slice(1).split('/').map((token) => {
    if (/~(?:[^01]|$)/.test(token)) throw new Error('invalid escape')
    return token.replace(/~[01]/g, (escape) => escape === '~1' ? '/' : '~')
  })
}
`,
  'src/value.ts': `import type { JsonValue } from './types.ts'
export function clone(value: JsonValue): JsonValue {
  if (value === null || typeof value !== 'object') return value
  if (Array.isArray(value)) return value.map(clone)
  return Object.fromEntries(Object.keys(value).map((key) => [key, clone(value[key])]))
}
export function equal(left: JsonValue, right: JsonValue): boolean {
  if (left === right) return true
  if (left === null || right === null || typeof left !== 'object' || typeof right !== 'object') return false
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length
      && left.every((value, index) => equal(value, right[index]))
  }
  const keys = Object.keys(left)
  return keys.length === Object.keys(right).length
    && keys.every((key) => Object.hasOwn(right, key) && equal(left[key], right[key]))
}
`,
  'src/patch.ts': `import { parsePointer } from './pointer.ts'
import { clone, equal } from './value.ts'
import type { JsonValue, Operation } from './types.ts'

type Container = JsonValue[] | { [key: string]: JsonValue }
function container(value: JsonValue): Container {
  if (value === null || typeof value !== 'object') throw new Error('not a container')
  return value
}
function index(token: string, length: number, insert: boolean, append: boolean): number {
  if (token === '-' && insert && append) return length
  if (!/^(0|[1-9][0-9]*)$/.test(token)) throw new Error('invalid index')
  const n = Number(token)
  if (!Number.isSafeInteger(n) || (insert ? n > length : n >= length)) throw new Error('out of bounds')
  return n
}
function read(document: JsonValue, tokens: readonly string[]): JsonValue {
  let current = document
  for (const token of tokens) {
    const parent = container(current)
    if (Array.isArray(parent)) current = parent[index(token, parent.length, false, false)]
    else {
      if (!Object.hasOwn(parent, token)) throw new Error('missing key')
      current = parent[token]
    }
  }
  return current
}
function edit(document: JsonValue, tokens: readonly string[], mode: 'add' | 'replace' | 'remove', value: JsonValue, append = false): JsonValue {
  if (tokens.length === 0) {
    if (mode === 'remove') throw new Error('cannot remove root')
    return value
  }
  const parent = container(read(document, tokens.slice(0, -1)))
  const token = tokens[tokens.length - 1]
  if (Array.isArray(parent)) {
    const n = index(token, parent.length, mode === 'add', append)
    if (mode === 'add') parent.splice(n, 0, value)
    else if (mode === 'remove') parent.splice(n, 1)
    else parent[n] = value
  } else {
    if (mode !== 'add' && !Object.hasOwn(parent, token)) throw new Error('missing key')
    if (mode === 'remove') delete parent[token]
    else Object.defineProperty(parent, token, { value, writable: true, configurable: true, enumerable: true })
  }
  return document
}
export function applyPatch(document: JsonValue, operations: readonly Operation[]): JsonValue {
  let result = clone(document)
  for (const operation of operations) {
    const path = parsePointer(operation.path)
    switch (operation.op) {
      case 'add':
      case 'replace':
        result = edit(result, path, operation.op, clone(operation.value), operation.op === 'add')
        break
      case 'remove':
        result = edit(result, path, 'remove', null)
        break
      case 'test':
        if (!equal(read(result, path), operation.value)) throw new Error('test failed')
        break
      case 'copy':
      case 'move': {
        const from = parsePointer(operation.from)
        const value = read(result, from)
        if (operation.op === 'move') {
          if (path.length > from.length && from.every((token, n) => token === path[n])) throw new Error('descendant move')
          if (path.length === from.length && from.every((token, n) => token === path[n])) break
          result = edit(result, from, 'remove', null)
        }
        result = edit(result, path, 'add', clone(value))
        break
      }
    }
  }
  return result
}
`,
}

const task = jsonPatchTasks[0]!
function grade(sources: Record<string, string> = {}): Grade {
  const root = mkdtempSync(join(tmpdir(), 'crabcode-json-patch-test-'))
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
function replaceOnce(source: string, before: string, after: string): string {
  expect(source.split(before)).toHaveLength(2)
  return source.replace(before, after)
}
function mutated(file: string, before: string, after: string): Grade {
  return grade({ ...reference, [file]: replaceOnce(reference[file]!, before, after) })
}
function expectRejected(result: Grade, suite: string) {
  expect(result.passed).toBe(false)
  const check = result.checks.find(({ name }) => name === suite)!
  expect(check.passed, check.output).toBe(false)
  expect(check.output).toContain('expect(')
}

test('exports one harness-neutral hard fixture with immutable visible and hidden checks', () => {
  expect(jsonPatchTasks).toHaveLength(1)
  expect(task.id).toBe('json-patch')
  expect(task.difficulty).toBe('hard')
  expect(Object.keys(task.checks)).toHaveLength(3)
  expect(task.files['tests/patch.test.ts']).toBe(task.checks['tests/patch.test.ts'])
  expect(Object.keys(task.files).filter((name) => name.startsWith('src/'))).toHaveLength(5)
})

test('compiling starter fails assertions; legitimate multi-module reference passes all suites', () => {
  const baseline = grade()
  expect(baseline.passed).toBe(false)
  for (const check of baseline.checks) {
    expect(check.passed, check.output).toBe(false)
    expect(check.output).toContain('expect(')
  }
  const good = grade(reference)
  expect(good.passed, JSON.stringify(good.checks)).toBe(true)
  for (const check of good.checks) expect(check.output).toContain('0 fail')
})

test('hidden grader rejects shallow copies even with correct operations', () => {
  const bad = mutated(
    'src/value.ts',
    "if (value === null || typeof value !== 'object') return value",
    'return value\n  // unreachable',
  )
  expectRejected(bad, 'tests/patch.semantics.hidden.test.ts')
})

test('hidden grader rejects stringify equality and loose index spelling', () => {
  const equality = mutated(
    'src/value.ts',
    'if (left === right) return true',
    'return JSON.stringify(left) === JSON.stringify(right)\n  // unreachable',
  )
  expectRejected(equality, 'tests/patch.semantics.hidden.test.ts')
  const indices = mutated('src/patch.ts', 'if (!/^(0|[1-9][0-9]*)$/.test(token))', 'if (!/^\\d+$/.test(token))')
  expectRejected(indices, 'tests/patch.paths.hidden.test.ts')
})

test('hidden grader rejects raw prefix ancestry and pre-removal move destinations', () => {
  const ancestry = mutated(
    'src/patch.ts',
    'path.length > from.length && from.every((token, n) => token === path[n])',
    'operation.path !== operation.from && operation.path.startsWith(operation.from)',
  )
  expectRejected(ancestry, 'tests/patch.semantics.hidden.test.ts')
  const shifted = mutated(
    'src/patch.ts',
    "result = edit(result, from, 'remove', null)",
    "result = edit(result, path, 'add', clone(value))\n          result = edit(result, from, 'remove', null)\n          break",
  )
  expectRejected(shifted, 'tests/patch.semantics.hidden.test.ts')
})

test('hidden grader rejects unsafe __proto__ assignment and inherited traversal', () => {
  const unsafe = mutated(
    'src/patch.ts',
    'Object.defineProperty(parent, token, { value, writable: true, configurable: true, enumerable: true })',
    'parent[token] = value',
  )
  expectRejected(unsafe, 'tests/patch.paths.hidden.test.ts')
  const inherited = mutated('src/patch.ts', 'if (!Object.hasOwn(parent, token))', 'if (!(token in parent))')
  expectRejected(inherited, 'tests/patch.paths.hidden.test.ts')
})
