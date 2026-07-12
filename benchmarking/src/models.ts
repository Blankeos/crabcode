import type { BenchmarkTask } from './types.ts'

/** Primary OpenAI coding model for most benchmark tasks (replaces deprecated gpt-5.3-codex). */
export const BENCHMARK_MODEL_CODEX_SPARK = 'openai/gpt-5.3-codex-spark'

/** Lighter model for simple tasks when spark is not appropriate (optional per-task override). */
export const BENCHMARK_MODEL_MINI = 'openai/gpt-5.4-mini'

/** Heavier model for the few hardest benchmark tasks. */
export const BENCHMARK_MODEL_HARD = 'openai/gpt-5.5'

/** Default when `--model` / `BENCH_MODEL` is not set. */
export const DEFAULT_MODEL = BENCHMARK_MODEL_CODEX_SPARK

const HARD_TASK_IDS = new Set(['workflow-planner-ts', 'issue-triage-pipeline-ts'])

/**
 * Picks the model for a task. Explicit CLI/env `modelOverride` wins.
 * Otherwise: hard tasks → gpt-5.5, everything else → gpt-5.3-codex-spark.
 */
export function resolveBenchmarkModel(task: BenchmarkTask, modelOverride?: string): string {
  const override = modelOverride?.trim()
  if (override) return override
  if (task.model) return task.model
  if (task.difficulty === 'hard' || HARD_TASK_IDS.has(task.id)) {
    return BENCHMARK_MODEL_HARD
  }
  return DEFAULT_MODEL
}