import { expect, test } from 'bun:test'
import { BENCHMARK_MODEL_CODEX_SPARK, BENCHMARK_MODEL_HARD, resolveBenchmarkModel } from './models.ts'
import type { BenchmarkTask } from './types.ts'

const baseTask = (overrides: Partial<BenchmarkTask>): BenchmarkTask => ({
  id: 'test-task',
  title: 't',
  prompt: 'p',
  files: {},
  check: () => [],
  ...overrides,
})

test('resolveBenchmarkModel uses spark by default', () => {
  expect(resolveBenchmarkModel(baseTask({ difficulty: 'smoke' }))).toBe(BENCHMARK_MODEL_CODEX_SPARK)
  expect(resolveBenchmarkModel(baseTask({ difficulty: 'medium' }))).toBe(BENCHMARK_MODEL_CODEX_SPARK)
})

test('resolveBenchmarkModel uses gpt-5.5 for hard tasks', () => {
  expect(resolveBenchmarkModel(baseTask({ difficulty: 'hard' }))).toBe(BENCHMARK_MODEL_HARD)
  expect(resolveBenchmarkModel(baseTask({ id: 'workflow-planner-ts' }))).toBe(BENCHMARK_MODEL_HARD)
  expect(resolveBenchmarkModel(baseTask({ id: 'issue-triage-pipeline-ts' }))).toBe(BENCHMARK_MODEL_HARD)
})

test('resolveBenchmarkModel respects override and task.model', () => {
  expect(resolveBenchmarkModel(baseTask({}), 'openai/custom')).toBe('openai/custom')
  expect(resolveBenchmarkModel(baseTask({ model: 'openai/task-only' }))).toBe('openai/task-only')
  expect(resolveBenchmarkModel(baseTask({ difficulty: 'hard', model: 'openai/task-only' }), 'openai/override')).toBe(
    'openai/override',
  )
})