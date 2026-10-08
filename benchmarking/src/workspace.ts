import { lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { dirname, join, relative, resolve } from 'node:path'
import { DEFAULT_BENCHMARK_DIR } from './defaults.ts'
import { sanitizePathPart } from './format.ts'
import type { BenchmarkTask, Command, GradedFixture, ProcessResult } from './types.ts'

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
  writeFiles(workspace, task.files)
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

export function writeFiles(root: string, files: Record<string, string>) {
  for (const [path, content] of Object.entries(files)) {
    const dest = join(root, path)
    mkdirSync(dirname(dest), { recursive: true })
    writeFileSync(dest, content)
  }
}

export function fixtureHash(task: GradedFixture) {
  return createHash('sha256').update(JSON.stringify(task)).digest('hex')
}

// Copy only source trees present in the fixture, including new helper modules.
// No candidate tests, symlinks, package.json, preload hooks or personal config.
export function candidateSources(task: GradedFixture, workspace: string): Record<string, string> {
  const roots = new Set(
    Object.keys(task.files)
      .filter(isSource)
      .map((path) => dirname(path)),
  )
  const files: Record<string, string> = {}
  function visit(dir: string) {
    if (!lstatSync(dir, { throwIfNoEntry: false })?.isDirectory()) return
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isSymbolicLink() || entry.name.startsWith('.') || entry.name === 'node_modules') continue
      const path = join(dir, entry.name)
      if (entry.isDirectory()) visit(path)
      else if (entry.isFile() && isSource(path)) files[relative(workspace, path)] = readFileSync(path, 'utf8')
    }
  }
  for (const root of roots) visit(join(workspace, root))
  return files
}

function isSource(path: string) {
  return /\.(ts|js|mts|mjs)$/.test(path) && !/\.(test|spec)\.[^.]+$/.test(path)
}

export function runProcess(
  command: Command,
  cwd: string,
  timeoutMs: number,
  env: NodeJS.ProcessEnv = {},
  signal?: AbortSignal,
): Promise<ProcessResult> {
  return new Promise((done) => {
    const started = performance.now()
    const child = spawn(command.bin, command.args, {
      cwd,
      env: { ...process.env, NO_COLOR: '1', CI: '1', ...env },
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let stdout = '',
      stderr = '',
      timedOut = false,
      interrupted = false,
      error: string | undefined
    let killTimer: ReturnType<typeof setTimeout> | undefined
    let settled = false
    function kill(sig: NodeJS.Signals) {
      if (!child.pid) return
      try {
        if (process.platform === 'win32') child.kill(sig)
        else process.kill(-child.pid, sig)
      } catch {}
    }
    function stop() {
      kill('SIGTERM')
      killTimer ??= setTimeout(() => kill('SIGKILL'), 1_000)
    }
    function abort() {
      interrupted = true
      stop()
    }
    const timer = setTimeout(() => {
      timedOut = true
      stop()
    }, timeoutMs)
    signal?.addEventListener('abort', abort, { once: true })
    if (signal?.aborted) abort()
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString()
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString()
    })
    child.on('error', (err) => {
      error = err.message
    })
    child.on('close', (exitCode) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      if (killTimer) {
        clearTimeout(killTimer)
        kill('SIGKILL')
      }
      signal?.removeEventListener('abort', abort)
      done({
        exitCode,
        timedOut,
        interrupted,
        elapsedMs: Math.round(performance.now() - started),
        stdout,
        stderr,
        error,
      })
    })
  })
}

export function sourceDiff(task: GradedFixture, workspace: string) {
  const files = candidateSources(task, workspace)
  return Object.fromEntries(
    [...new Set([...Object.keys(task.files).filter(isSource), ...Object.keys(files)])]
      .filter((path) => task.files[path] !== files[path])
      .map((path) => [path, { before: task.files[path] ?? null, after: files[path] ?? null }]),
  )
}
