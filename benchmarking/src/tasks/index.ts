import { codingTasks } from './coding.ts'
import { asyncCacheTasks } from './async-cache.ts'
import { jsonPatchTasks } from './json-patch.ts'
import { workflowTasks } from './workflow.ts'
import { gradeTask } from '../checks.ts'
import type { BenchmarkTask } from '../types.ts'
import { basicTasks } from './basic.ts'
import { rustTasks } from './rust.ts'
import { siteTasks } from './site.ts'
import { triageTasks } from './triage.ts'
import { typescriptTasks } from './typescript.ts'

export const DEFAULT_TASKS = [...basicTasks, ...rustTasks, ...siteTasks, ...typescriptTasks, ...triageTasks]
export const GRADED_TASKS: BenchmarkTask[] = [
  ...codingTasks,
  ...workflowTasks,
  ...asyncCacheTasks,
  ...jsonPatchTasks,
].map((fixture) => ({
  ...fixture,
  defaultEnabled: false,
  difficulty: fixture.difficulty ?? 'smoke',
  tags: ['typescript', 'hidden-tests', 'harness'],
  grader: fixture,
  check: (cwd) =>
    gradeTask(fixture, cwd).checks.map((check) => ({
      name: check.name,
      pass: check.passed,
      detail: check.output,
    })),
}))

export const TASKS = [...DEFAULT_TASKS, ...GRADED_TASKS]
