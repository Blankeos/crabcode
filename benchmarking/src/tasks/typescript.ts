import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { bunTestCheck, bunTestWithHiddenFileCheck } from '../checks.ts'
import { BENCHMARK_MODEL_CODEX_SPARK } from '../models.ts'
import { defineTask } from './define.ts'

const FIXTURE_BENCHMARK_MODEL = BENCHMARK_MODEL_CODEX_SPARK

export const typescriptTasks = [
  defineTask({
    id: 'invoice-ts-fix',
    title: 'Fix a cross-file TypeScript invoice bug',
    difficulty: 'medium',
    tags: ['typescript', 'bugfix', 'tests'],
    files: {
      'package.json': JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n',
      'src/invoice.ts': `export type InvoiceLine = {
  sku: string
  unitCents: number
  quantity: number
}

export function invoiceTotalCents(lines: InvoiceLine[], discountPercent = 0, taxRate = 0): number {
  const subtotal = lines.reduce((sum, line) => sum + line.unitCents, 0)
  const discounted = subtotal - Math.round(subtotal * discountPercent)
  return Math.round(discounted * (1 + taxRate))
}
`,
      'tests/invoice.test.ts': `import { expect, test } from 'bun:test'
import { invoiceTotalCents } from '../src/invoice'

test('counts quantities before discount and tax', () => {
  const total = invoiceTotalCents(
    [
      { sku: 'seat', unitCents: 1000, quantity: 2 },
      { sku: 'addon', unitCents: 500, quantity: 1 },
    ],
    10,
    0.08,
  )

  expect(total).toBe(2430)
})

test('handles quantity-only totals', () => {
  expect(invoiceTotalCents([{ sku: 'usage', unitCents: 333, quantity: 3 }])).toBe(999)
})
`,
    },
    prompt: `Fix src/invoice.ts so invoiceTotalCents counts line quantities, treats discountPercent as a whole percent where 10 means 10%, and keeps taxRate as a decimal. Do not change the tests or add dependencies.`,
    check: (cwd) => {
      const testFile = readFileSync(join(cwd, 'tests/invoice.test.ts'), 'utf8')
      return [
        { name: 'keeps invoice behavior tests', pass: testFile.includes('toBe(2430)') && testFile.includes('quantity: 3') },
        bunTestCheck(cwd),
      ]
    },
  }),
  defineTask({
    id: 'jsonc-config-parser',
    title: 'Add tiny JSONC config parser support',
    difficulty: 'medium',
    tags: ['typescript', 'parser', 'jsonc'],
    files: {
      'package.json': JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n',
      'src/config.ts': `export type AppConfig = {
  model: string
  limits: {
    maxTurns: number
  }
  features: string[]
}

export function parseConfig(text: string): AppConfig {
  return JSON.parse(text)
}
`,
      'tests/config.test.ts': `import { expect, test } from 'bun:test'
import { parseConfig } from '../src/config'

test('parses line comments and trailing commas', () => {
  const config = parseConfig(\`{
    // default benchmark model
    "model": "${FIXTURE_BENCHMARK_MODEL}",
    "limits": {
      "maxTurns": 8,
    },
    "features": [
      "shell",
      "edit",
    ],
  }\`)

  expect(config).toEqual({
    model: '${FIXTURE_BENCHMARK_MODEL}',
    limits: { maxTurns: 8 },
    features: ['shell', 'edit'],
  })
})
`,
    },
    prompt: `Update src/config.ts so parseConfig accepts JSONC-style // line comments and trailing commas in objects/arrays. Keep the public API the same, keep the existing test, and do not add dependencies.`,
    check: (cwd) => {
      const testFile = readFileSync(join(cwd, 'tests/config.test.ts'), 'utf8')
      return [
        {
          name: 'keeps JSONC coverage',
          pass: testFile.includes('// default benchmark model') && testFile.includes('"maxTurns": 8,') && testFile.includes('"edit",'),
        },
        bunTestCheck(cwd),
      ]
    },
  }),
  defineTask({
    id: 'workflow-planner-ts',
    title: 'Implement dependency-aware workflow planning',
    difficulty: 'hard',
    tags: ['typescript', 'algorithm', 'hidden-tests'],
    timeoutMs: 60_000,
    files: {
      'package.json': JSON.stringify({ type: 'module', scripts: { test: 'bun test' } }, null, 2) + '\n',
      'src/planner.ts': `export type WorkflowStep = {
  id: string
  dependsOn?: string[]
  estimatedSeconds?: number
}

export type ExecutionStage = {
  parallel: string[]
}

export function createExecutionPlan(steps: WorkflowStep[]): ExecutionStage[] {
  return steps.map((step) => ({ parallel: [step.id] }))
}
`,
      'tests/planner.test.ts': `import { expect, test } from 'bun:test'
import { createExecutionPlan, type WorkflowStep } from '../src/planner'

test('groups ready steps into stable dependency stages', () => {
  const steps: WorkflowStep[] = [
    { id: 'checkout' },
    { id: 'lint', dependsOn: ['checkout'] },
    { id: 'docs', dependsOn: ['checkout'] },
    { id: 'test', dependsOn: ['lint'] },
    { id: 'package', dependsOn: ['docs', 'test'] },
  ]

  expect(createExecutionPlan(steps)).toEqual([
    { parallel: ['checkout'] },
    { parallel: ['lint', 'docs'] },
    { parallel: ['test'] },
    { parallel: ['package'] },
  ])
})

test('rejects missing dependencies with useful context', () => {
  expect(() =>
    createExecutionPlan([
      { id: 'deploy', dependsOn: ['package'] },
    ]),
  ).toThrow(/package.*deploy|deploy.*package/)
})

test('rejects dependency cycles', () => {
  expect(() =>
    createExecutionPlan([
      { id: 'a', dependsOn: ['b'] },
      { id: 'b', dependsOn: ['a'] },
    ]),
  ).toThrow(/cycle/i)
})
`,
    },
    prompt: `Implement createExecutionPlan in src/planner.ts. Return execution stages where every step in a stage can run after all previous stages, and keep the original input order inside each stage. The function must handle input that is not already sorted, throw helpful errors for duplicate step ids, unknown dependencies, and dependency cycles, and it must not mutate the input. Do not change the tests or add dependencies.`,
    check: (cwd) => {
      const testFile = readFileSync(join(cwd, 'tests/planner.test.ts'), 'utf8')
      return [
        {
          name: 'keeps visible planner coverage',
          pass:
            testFile.includes('groups ready steps into stable dependency stages') &&
            testFile.includes('rejects missing dependencies') &&
            testFile.includes('rejects dependency cycles'),
        },
        bunTestWithHiddenFileCheck(
          cwd,
          'hidden workflow planner tests pass',
          'tests/__bench_hidden_planner.test.ts',
          `import { expect, test } from 'bun:test'
import { createExecutionPlan, type WorkflowStep } from '../src/planner'

test('plans unsorted dependency input without mutating it', () => {
  const steps: WorkflowStep[] = [
    { id: 'deploy', dependsOn: ['build', 'migrate'], estimatedSeconds: 30 },
    { id: 'lint', estimatedSeconds: 10 },
    { id: 'build', dependsOn: ['lint'], estimatedSeconds: 40 },
    { id: 'migrate', dependsOn: ['lint'], estimatedSeconds: 15 },
    { id: 'notify', dependsOn: ['deploy'], estimatedSeconds: 5 },
  ]
  const original = structuredClone(steps)

  expect(createExecutionPlan(steps)).toEqual([
    { parallel: ['lint'] },
    { parallel: ['build', 'migrate'] },
    { parallel: ['deploy'] },
    { parallel: ['notify'] },
  ])
  expect(steps).toEqual(original)
})

test('rejects duplicate step ids', () => {
  expect(() =>
    createExecutionPlan([
      { id: 'build' },
      { id: 'build', dependsOn: ['build'] },
    ]),
  ).toThrow(/duplicate|build/i)
})

test('detects cycles even when independent work is present', () => {
  expect(() =>
    createExecutionPlan([
      { id: 'setup' },
      { id: 'a', dependsOn: ['b'] },
      { id: 'b', dependsOn: ['a'] },
    ]),
  ).toThrow(/cycle/i)
})
`,
        ),
      ]
    },
  }),
]
