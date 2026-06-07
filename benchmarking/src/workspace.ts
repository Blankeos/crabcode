import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { DEFAULT_BENCHMARK_DIR } from './defaults.ts'
import { sanitizePathPart } from './format.ts'
import type { BenchmarkTask } from './types.ts'

export function createRunRoot(dir: string | boolean | undefined, runId: string) {
  const parent = dir && dir !== true ? resolve(String(dir)) : DEFAULT_BENCHMARK_DIR
  mkdirSync(parent, { recursive: true })
  const root = join(parent, runId)
  mkdirSync(root, { recursive: true })
  return root
}

export function timestampForPath() {
  return new Date().toISOString().replaceAll(':', '').replaceAll('.', '-')
}

export function writeFixture(workspace: string, task: BenchmarkTask) {
  for (const [path, content] of Object.entries(task.files)) {
    const fullPath = join(workspace, path)
    mkdirSync(dirname(fullPath), { recursive: true })
    writeFileSync(fullPath, content)
  }
}

export function writeRunArtifacts(logsRoot: string, runLabel: string, command: string, stdout: string, stderr: string) {
  const safeLabel = sanitizePathPart(runLabel)
  const commandPath = join(logsRoot, `${safeLabel}.command.txt`)
  const stdoutPath = join(logsRoot, `${safeLabel}.stdout.txt`)
  const stderrPath = join(logsRoot, `${safeLabel}.stderr.txt`)

  writeFileSync(commandPath, command + '\n')
  writeFileSync(stdoutPath, stdout)
  writeFileSync(stderrPath, stderr)

  return { commandPath, stdoutPath, stderrPath }
}

export function cleanupWorkspace(workspace: string) {
  try {
    rmSync(workspace, { recursive: true, force: true })
  } catch {}
}

export function cleanupWorkspaceChildren(workspace: string) {
  try {
    for (const entry of readdirSync(workspace)) {
      cleanupWorkspace(join(workspace, entry))
    }
  } catch {}
}

