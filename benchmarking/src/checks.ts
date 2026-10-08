import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { tailText } from './format.ts'
import type { BenchmarkTask, CheckResult, Grade, GradedFixture } from './types.ts'
import { candidateSources, writeFiles } from './workspace.ts'

export function runChecks(task: BenchmarkTask, workspace: string): CheckResult[] {
  try {
    return task.check(workspace)
  } catch (err) {
    return [
      {
        name: 'checks completed',
        pass: false,
        detail: err instanceof Error ? err.message : String(err),
      },
    ]
  }
}

export function bunTestCheck(cwd: string): CheckResult {
  const result = runCheckCommand(cwd, process.execPath, ['test'])
  return {
    name: 'bun test passes',
    pass: result.ok,
    detail: result.detail,
  }
}

export function bunTestWithHiddenFileCheck(cwd: string, name: string, path: string, content: string): CheckResult {
  const fullPath = join(cwd, path)
  mkdirSync(dirname(fullPath), { recursive: true })
  writeFileSync(fullPath, content)

  const result = runCheckCommand(cwd, process.execPath, ['test'])
  return {
    name,
    pass: result.ok,
    detail: result.detail,
  }
}

export function runCheckCommand(cwd: string, command: string, args: string[]) {
  const proc = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    timeout: 15_000,
    env: {
      ...process.env,
      NO_COLOR: '1',
      CI: '1',
    },
  })
  const output = `${proc.stdout ?? ''}\n${proc.stderr ?? ''}`.trim()
  const detail = proc.error
    ? proc.error.message
    : proc.status === 0
      ? undefined
      : tailText(output, 600) || `exit code ${proc.status}`

  return {
    ok: proc.status === 0,
    detail,
  }
}

export function gradeTask(task: GradedFixture, workspace: string): Grade {
  const root = mkdtempSync(join(tmpdir(), 'crabcode-harness-grade-'))
  try {
    writeFiles(root, candidateSources(task, workspace))
    writeFiles(root, { 'package.json': '{"type":"module"}\n', ...task.checks })
    const checks = Object.keys(task.checks).map((path) => {
      const result = spawnSync(process.execPath, ['test', `./${path}`], {
        cwd: root,
        encoding: 'utf8',
        timeout: 10_000,
        env: { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: root, NO_COLOR: '1', CI: '1' },
      })
      return {
        name: path,
        passed: result.status === 0 && !result.error,
        output: (result.error?.message || `${result.stdout ?? ''}\n${result.stderr ?? ''}`).slice(-12_000),
      }
    })
    return { passed: checks.length > 0 && checks.every((check) => check.passed), checks }
  } catch (error) {
    return { passed: false, checks: [{ name: 'grader', passed: false, output: String(error) }] }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}
