import { spawnSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { tailText } from './format.ts'
import type { BenchmarkTask, CheckResult } from './types.ts'

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

