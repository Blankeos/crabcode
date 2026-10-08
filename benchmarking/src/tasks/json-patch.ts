import type { GradedFixture } from '../types.ts'

const prelude = `import { expect, test } from 'bun:test'
import { applyPatch, type JsonValue, type Operation } from '../src/index.ts'

function freeze<T>(value: T): T {
  if (value !== null && typeof value === 'object') {
    for (const child of Object.values(value)) freeze(child)
    Object.freeze(value)
  }
  return value
}
function reject(document: JsonValue, operations: readonly Operation[]) {
  const before = JSON.stringify(document)
  const patchBefore = JSON.stringify(operations)
  expect(() => applyPatch(document, operations)).toThrow(Error)
  expect(JSON.stringify(document)).toBe(before)
  expect(JSON.stringify(operations)).toBe(patchBefore)
}
`

const visible = `${prelude}
test('applies ordered edits with escaped keys and shifting arrays', () => {
  const document = freeze({ 'a/b': { '~key': [1, 2, 3] }, keep: null })
  const patch: readonly Operation[] = freeze([
    { op: 'add', path: '/a~1b/~0key/1', value: 9 },
    { op: 'remove', path: '/a~1b/~0key/2' },
    { op: 'replace', path: '/keep', value: false },
    { op: 'add', path: '/a~1b/~0key/-', value: 4 },
    { op: 'test', path: '/a~1b/~0key', value: [1, 9, 3, 4] },
  ])
  expect(applyPatch(document, patch)).toEqual({ 'a/b': { '~key': [1, 9, 3, 4] }, keep: false })
  expect(document).toEqual({ 'a/b': { '~key': [1, 2, 3] }, keep: null })
})
test('copies deeply and interprets same-array move destination after removal', () => {
  const result = applyPatch({ a: [{ n: 1 }, { n: 2 }, { n: 3 }], b: null }, [
    { op: 'copy', from: '/a/0', path: '/b' },
    { op: 'replace', path: '/b/n', value: 8 },
    { op: 'move', from: '/a/0', path: '/a/2' },
  ])
  expect(result).toEqual({ a: [{ n: 2 }, { n: 3 }, { n: 1 }], b: { n: 8 } })
})
test('failure is atomic, test uses structural equality and missing is not null', () => {
  const document = { a: { x: 1, y: [null, false] } }
  expect(applyPatch(document, [{ op: 'test', path: '/a', value: { y: [null, false], x: 1 } }])).toEqual(document)
  reject(document, [{ op: 'replace', path: '/a/x', value: 4 }, { op: 'test', path: '/a/x', value: 1 }])
  reject(document, [{ op: 'test', path: '/missing', value: null }])
})
`

const pathsHidden = `${prelude}
test('strict pointer syntax, one-pass decoding, empty tokens and no URI decoding', () => {
  const document = { '': { '': 1 }, '~1': 2, '%2F': 3, 'a/b': 4, '~': 5 }
  for (const [path, value] of [['//', 1], ['/~01', 2], ['/%2F', 3], ['/a~1b', 4], ['/~0', 5]] as const) {
    expect(applyPatch(document, [{ op: 'test', path, value }])).toEqual(document)
  }
  for (const path of ['a', '#/a', '/~', '/~2', '/x~9', '/~~0']) {
    reject(document, [{ op: 'add', path, value: 0 }])
    reject(document, [{ op: 'copy', from: path, path: '/new' }])
    reject(document, [{ op: 'move', from: path, path: '/new' }])
  }
})
test('array tokens are canonical and bounds depend on operation', () => {
  const document = { a: [null, 1], '01': 7, '-': 8 }
  for (const token of ['01', '00', '+1', '-1', '1.0', '1e0', ' 1', '1 ', '9007199254740993', 'length', 'constructor']) {
    for (const op of ['add', 'replace', 'test'] as const) reject(document, [{ op, path: '/a/' + token, value: 0 }])
    reject(document, [{ op: 'remove', path: '/a/' + token }])
  }
  expect(applyPatch(document, [{ op: 'add', path: '/a/2', value: 2 }])).toEqual({ ...document, a: [null, 1, 2] })
  reject(document, [{ op: 'add', path: '/a/3', value: 2 }])
  for (const path of ['/a/2', '/a/-']) {
    reject(document, [{ op: 'remove', path }])
    reject(document, [{ op: 'replace', path, value: 0 }])
    reject(document, [{ op: 'test', path, value: null }])
    reject(document, [{ op: 'copy', from: path, path: '/new' }])
    reject(document, [{ op: 'move', from: path, path: '/new' }])
  }
  for (const op of ['copy', 'move'] as const) reject(document, [{ op, from: '/a/0', path: '/a/-' }])
  expect(applyPatch(document, [{ op: 'replace', path: '/01', value: 9 }, { op: 'test', path: '/-', value: 8 }])).toEqual({ ...document, '01': 9 })
  expect(applyPatch([], [{ op: 'add', path: '/0', value: null }])).toEqual([null])
})
test('existing own parents only, null versus absence, no implicit containers', () => {
  const document = { a: null, b: 0, c: false, d: 'text', nested: {}, array: [] }
  for (const path of ['/absent/x', '/a/x', '/b/x', '/c/x', '/d/0', '/array/0/x', '/nested/missing/x']) {
    reject(document, [{ op: 'add', path, value: 1 }])
  }
  for (const path of ['/absent', '/nested/missing']) {
    reject(document, [{ op: 'replace', path, value: null }])
    reject(document, [{ op: 'remove', path }])
    reject(document, [{ op: 'copy', from: path, path: '/new' }])
    reject(document, [{ op: 'move', from: path, path }])
  }
  expect(applyPatch(document, [{ op: 'test', path: '/a', value: null }, { op: 'add', path: '/a', value: 2 }])).toEqual({ ...document, a: 2 })
})
test('prototype names are ordinary own JSON keys and cannot access inherited properties', () => {
  const document = JSON.parse('{"__proto__":{"safe":1},"constructor":{"prototype":{"safe":2}},"toString":3}')
  const result = applyPatch(document, [
    { op: 'copy', from: '/__proto__', path: '/copied' },
    { op: 'replace', path: '/__proto__/safe', value: 4 },
    { op: 'remove', path: '/toString' },
    { op: 'add', path: '/toString', value: 5 },
    { op: 'test', path: '/constructor/prototype/safe', value: 2 },
  ])
  expect(result).toEqual(JSON.parse('{"__proto__":{"safe":4},"constructor":{"prototype":{"safe":2}},"toString":5,"copied":{"safe":1}}'))
  expect(Object.hasOwn(result as object, '__proto__')).toBe(true)
  const added = applyPatch({}, [{ op: 'add', path: '/__proto__', value: { safe: 7 } }])
  expect(Object.hasOwn(added as object, '__proto__')).toBe(true)
  expect(JSON.stringify(added)).toBe('{"__proto__":{"safe":7}}')
  for (const path of ['/__proto__', '/constructor', '/toString', '/hasOwnProperty']) {
    reject({}, [{ op: 'test', path, value: null }])
    reject({}, [{ op: 'add', path: path + '/polluted', value: true }])
  }
  expect(Object.hasOwn(Object.prototype, 'polluted')).toBe(false)
})
`

const semanticsHidden = `${prelude}
test('root replacement, copy, test and forbidden root removal', () => {
  for (const document of [null, false, 0, '', [], {}] as JsonValue[]) {
    expect(applyPatch(document, [{ op: 'add', path: '', value: [1] }])).toEqual([1])
    expect(applyPatch(document, [{ op: 'replace', path: '', value: null }])).toBe(null)
    expect(applyPatch(document, [{ op: 'test', path: '', value: document }])).toEqual(document)
    reject(document, [{ op: 'remove', path: '' }])
    expect(applyPatch(document, [{ op: 'move', from: '', path: '' }])).toEqual(document)
  }
  expect(applyPatch({ a: [1] }, [{ op: 'copy', from: '/a', path: '' }])).toEqual([1])
  expect(applyPatch({ a: [1], b: 2 }, [{ op: 'move', from: '/a', path: '' }])).toEqual([1])
  expect(applyPatch({ a: 1 }, [{ op: 'copy', from: '', path: '/snapshot' }])).toEqual({ a: 1, snapshot: { a: 1 } })
})
test('move checks decoded token ancestry, not raw string prefixes', () => {
  expect(applyPatch({ a: 1, ab: {} }, [{ op: 'move', from: '/a', path: '/ab/x' }])).toEqual({ ab: { x: 1 } })
  expect(applyPatch({ 'a/b': 1, a: {} }, [{ op: 'move', from: '/a~1b', path: '/a/x' }])).toEqual({ a: { x: 1 } })
  for (const [from, path] of [['', '/x'], ['/a', '/a/x'], ['/a~1b', '/a~1b/x'], ['/a', '/a/']]) {
    reject({ a: { '': {}, x: {} }, 'a/b': { x: {} } }, [{ op: 'move', from, path }])
  }
  expect(applyPatch({ a: { x: 1 } }, [{ op: 'move', from: '/a', path: '/a' }])).toEqual({ a: { x: 1 } })
  reject({ a: 1 }, [{ op: 'move', from: '/missing', path: '/missing' }])
})
test('all same-array source and destination pairs use post-removal bounds', () => {
  for (let length = 1; length <= 6; length++) {
    for (let from = 0; from < length; from++) {
      for (let to = 0; to < length; to++) {
        const input = Array.from({ length }, (_, n) => n)
        const expected = [...input]
        const [value] = expected.splice(from, 1)
        expected.splice(to, 0, value)
        expect(applyPatch(input, [{ op: 'move', from: '/' + from, path: '/' + to }])).toEqual(expected)
        expect(input).toEqual(Array.from({ length }, (_, n) => n))
      }
    }
    reject(Array.from({ length }, (_, n) => n), [{ op: 'move', from: '/0', path: '/' + length }])
  }
  expect(applyPatch([0, 1, 2], [{ op: 'copy', from: '/0', path: '/2' }])).toEqual([0, 1, 0, 2])
  expect(applyPatch({ a: [0, 1], b: [] }, [{ op: 'move', from: '/a/1', path: '/b/0' }])).toEqual({ a: [0], b: [1] })
})
test('deep independent output, inserted values and copied siblings', () => {
  const document = freeze({ source: { nested: [1] }, untouched: { nested: [2] } })
  const value = freeze({ nested: [3] })
  const patch: readonly Operation[] = freeze([
    { op: 'copy', from: '/source', path: '/copy' },
    { op: 'add', path: '/inserted', value },
    { op: 'replace', path: '/replaced', value },
  ])
  // Replace requires existence; this also checks rollback after a successful copy/add.
  reject(document, patch)
  const result = applyPatch(document, patch.slice(0, 2)) as Record<string, JsonValue>
  ;((result.copy as Record<string, JsonValue>).nested as JsonValue[]).push(9)
  ;((result.inserted as Record<string, JsonValue>).nested as JsonValue[]).push(9)
  ;((result.untouched as Record<string, JsonValue>).nested as JsonValue[]).push(9)
  expect(result.source).toEqual({ nested: [1] })
  expect(document).toEqual({ source: { nested: [1] }, untouched: { nested: [2] } })
  expect(value).toEqual({ nested: [3] })
  const empty = applyPatch(document, []) as Record<string, JsonValue>
  expect(empty).not.toBe(document)
  expect(empty.source).not.toBe(document.source)
  const replaced = applyPatch({ a: null }, [{ op: 'replace', path: '/a', value }]) as Record<string, JsonValue>
  expect(replaced.a).not.toBe(value)
  const root = applyPatch(null, [{ op: 'replace', path: '', value }])
  expect(root).not.toBe(value)
})
test('equality preserves types, array order, object keys and nested nulls', () => {
  const pairs: [JsonValue, JsonValue][] = [
    [1, '1'], [false, 0], [null, {}], [[1, 2], [2, 1]], [[1], { '0': 1 }],
    [{ a: null }, {}], [{ a: 1 }, { a: 1, b: 2 }],
    [JSON.parse('{"__proto__":1}'), {}],
  ]
  for (const [document, value] of pairs) reject(document, [{ op: 'test', path: '', value }])
  expect(applyPatch({ a: [{ x: 1, y: null }], b: false }, [
    { op: 'test', path: '', value: { b: false, a: [{ y: null, x: 1 }] } },
  ])).toEqual({ a: [{ x: 1, y: null }], b: false })
})
test('late errors after every modifying operation leave document and patch untouched', () => {
  const document = { a: [1, 2], b: { x: null } }
  const first: Operation[] = [
    { op: 'add', path: '/a/1', value: {} }, { op: 'remove', path: '/a/0' },
    { op: 'replace', path: '/b', value: [] }, { op: 'copy', from: '/b', path: '/new' },
    { op: 'move', from: '/a/0', path: '/a/1' },
  ]
  for (const operation of first) reject(document, [operation, { op: 'remove', path: '/missing' }])
  reject(document, [{ op: 'move', from: '/b', path: '/missing/child' }])
})
`

const jsonPatchTask: GradedFixture = {
  id: 'json-patch',
  title: 'Repair an atomic JSON Patch engine',
  difficulty: 'hard',
  prompt: `Repair applyPatch(document, operations) exported by src/index.ts using the supplied
multi-module TypeScript engine. Preserve public types/API; helpers in src/ are allowed.
Implement the following explicit subset of RFC 6901/6902, synchronously, without dependencies.

Inputs conform to JsonValue and Operation: finite acyclic JSON trees, finite numbers,
dense arrays and objects with own enumerable string keys only (including JSON.parse-created
__proto__, constructor, prototype and toString). No undefined, special objects or runtime
validation of non-JSON inputs is required. Paths are strings, but may be invalid pointers.
Every invalid pointer, missing location, failed test or prohibited operation throws Error;
error messages are unspecified.

A pointer is empty (the document root) or starts with /. Split on / preserving empty tokens.
Decode ~1 to / and ~0 to ~ exactly once; reject every other ~ escape, including a trailing ~.
Do not URI-decode or accept URI fragments. Traverse own properties only, never prototypes.
All parents must already exist and be objects/arrays, not primitives or null. Object tokens
are literal keys; numeric spelling restrictions apply ONLY when addressing arrays.
Array indices must match 0 or [1-9][0-9]* and be safely representable integers. Existing
locations require index < length. Add permits index <= length and inserts (shifting items).
The '-' array token appends ONLY for op: add, never for other operations or source traversal.
This deliberately restricts copy/move destinations to numeric indices (object '-' is a normal key).

Apply operations in order against the current working document:
- add: set/overwrite an object member, insert into an array, or replace the entire root.
- remove: require an existing member/item and delete it (arrays shift). Root removal is forbidden.
- replace: require an existing location and set it, without inserting into arrays. Root always exists.
- test: require an existing location and compare recursively: object key order is irrelevant,
  key sets must match, arrays are ordered, primitive types/values must match. Missing is not null.
- copy: read an existing from location, deep-copy it and use add semantics at path, except '-'
  for arrays is forbidden as above. Source root and destination root are permitted.
- move: read an existing from, remove it, then use add semantics at path against the document
  AFTER removal (including same-array index shifts). A destination strictly beneath source is
  forbidden using decoded TOKEN ancestry, not raw string prefixes. Equal paths are no-ops
  but must resolve an existing source. Thus root-to-root move is allowed; root-to-descendant
  move is forbidden; nonroot-to-root move is allowed. Source removal otherwise follows remove.

The whole call is atomic: NEVER mutate document, operations or values inside operations,
including on failure. Return a deeply independent JSON tree, even for an empty patch or
only test/no-op operations. No mutable output subtree may alias input or patch values, and
copy must not alias its source. Support frozen inputs and prototype-named own keys safely.

Work only in your supplied workspace. Keep visible tests and run bun test. No installs,
network or external services. The clean-room grader copies only src/ source modules and
runs immutable visible/hidden tests; candidate tests, scripts and configuration cannot
influence grading.`,
  files: {
    'package.json': '{"type":"module","scripts":{"test":"bun test"}}\n',
    'README.md':
      '# JSON Patch repair\n\nRun `bun test`; no install required.\n\nThe compiling starter has intentionally broken pointer, value and engine modules.\nRepair src/pointer.ts (tokens), src/value.ts (cloning/equality), and src/patch.ts\n(ordered atomic operations). src/index.ts is the public API; src/types.ts defines inputs.\n',
    'src/types.ts': `export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
export type Operation =
  | { readonly op: 'add' | 'replace' | 'test'; readonly path: string; readonly value: JsonValue }
  | { readonly op: 'remove'; readonly path: string }
  | { readonly op: 'copy' | 'move'; readonly path: string; readonly from: string }
`,
    'src/index.ts': `export { applyPatch } from './patch.ts'
export type { JsonValue, Operation } from './types.ts'
`,
    'src/pointer.ts': `export function parsePointer(path: string): string[] {
  // TODO: validate RFC 6901 syntax and decode tokens without double-decoding.
  return path.split('/').slice(1)
}
`,
    'src/value.ts': `import type { JsonValue } from './types.ts'
export function clone(value: JsonValue): JsonValue {
  // TODO: independent deep JSON copy, including prototype-named own keys.
  return value
}
export function equal(left: JsonValue, right: JsonValue): boolean {
  // TODO: recursive structural equality, insensitive to object key order.
  return JSON.stringify(left) === JSON.stringify(right)
}
`,
    'src/patch.ts': `import { parsePointer } from './pointer.ts'
import { clone, equal } from './value.ts'
import type { JsonValue, Operation } from './types.ts'

export function applyPatch(document: JsonValue, operations: readonly Operation[]): JsonValue {
  // TODO: resolve paths and implement all operations on an independent working tree.
  let result = clone(document)
  for (const operation of operations) {
    parsePointer(operation.path)
    if (operation.op === 'test' && !equal(result, operation.value)) throw new Error('test failed')
    if (operation.op === 'add' || operation.op === 'replace') result = clone(operation.value)
  }
  return result
}
`,
    'tests/patch.test.ts': visible,
  },
  checks: {
    'tests/patch.test.ts': visible,
    'tests/patch.paths.hidden.test.ts': pathsHidden,
    'tests/patch.semantics.hidden.test.ts': semanticsHidden,
  },
}

// Export only: registration/opt-in selection belongs to the parent harness.
export const jsonPatchTasks: GradedFixture[] = [jsonPatchTask]
