import { expect, test } from 'bun:test'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import * as fs from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gradeTask } from './checks.ts'
import { selectTasks } from './cli.ts'
import { AVAILABLE_AGENTS, REPO_ROOT } from './defaults.ts'
import { shellQuote } from './format.ts'
import { codingTasks } from './tasks/coding.ts'
import { TASKS } from './tasks/index.ts'
import { workflowTasks } from './tasks/workflow.ts'
import { candidateSources, runProcess, sourceDiff, writeFiles } from './workspace.ts'

const runner = fileURLToPath(new URL('../bench-agents.ts', import.meta.url))
const page = codingTasks[0]
const solution = `export function page<T>(items: readonly T[], offset: number, limit: number): T[] {
  if (!Number.isInteger(offset) || offset < 0 || !Number.isInteger(limit) || limit < 0) throw new RangeError()
  return items.slice(offset, offset + limit)
}\n`
const statsSolution = `export function average(nums) {
  if (nums.length === 0) return 0
  return nums.reduce((s, n) => s + n, 0) / nums.length
}\n`
const event = (event: string, elapsed_ms: number, fields = {}) =>
  JSON.stringify({ schema_version: 1, event, elapsed_ms, ...fields })
const trace =
  [
    event('init', 0),
    event('model', 1, { model: 'model', provider: 'test', requested_effort: 'low', sent_effort: 'low' }),
    event('request_config', 2, { model: 'model-resolved', provider: 'test', sent_effort: 'low' }),
    event('provider_step_start', 10, { step: 1, messages: 2, tools: 6 }),
    event('first_response', 20, { step: 1, kind: 'tool_call' }),
    event('provider_end', 30, { step: 1, reason: 'tool_calls' }),
    event('response_summary', 31, { step: 1, reasoning_bytes: 0, text_bytes: 10 }),
    event('usage', 32, { input_tokens: 11, output_tokens: 13, cache_read_tokens: 3, cache_write_tokens: 0 }),
    event('tool_start', 35, { id: 'tool-1', name: 'edit', signature: 'deterministic-edit' }),
    event('tool_end', 45, { id: 'tool-1', name: 'edit', output_bytes: 2 }),
    event('step_finish', 46, { step: 1, action: 'finish' }),
    event('end', 50),
  ].join('\n') + '\n'
const fakeSource = `#!${process.execPath}
import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { isAbsolute, join } from 'node:path'
const args = process.argv.slice(2), agent = args.includes('--model') ? 'claude' : 'crabcode'
const model = args[args.indexOf(agent === 'claude' ? '--model' : '-m') + 1]
if (model !== (agent === 'claude' ? 'claude-haiku-4-5' : 'test/model')) throw new Error('wrong model or preflight')
if (!args.includes('--no-session-persistence')) throw new Error('conversation must be fresh')
if (JSON.parse(readFileSync('crabcode.jsonc', 'utf8')).model !== 'test/model') throw new Error('wrong config')
const attempt = existsSync('.fake-attempt') ? Number(readFileSync('.fake-attempt', 'utf8')) + 1 : 1
if (args.at(-1).includes('Verification after attempt') !== (attempt > 1)) throw new Error('wrong verifier feedback')
appendFileSync(process.env.FAKE_INVOCATIONS, JSON.stringify({ agent, args, cwd: process.cwd(), attempt }) + '\\n')
writeFileSync('.fake-attempt', String(attempt))
if (process.env.FAKE_MODE === 'wait') {
  writeFileSync(process.env.FAKE_READY, 'ready')
  setTimeout(() => process.exit(99), 7000)
  await new Promise(() => {})
}
// Both agents must rendezvous at every attempt: sequential execution cannot pass.
if (process.env.FAKE_BARRIER) {
  writeFileSync(join(process.env.FAKE_BARRIER, agent + '-' + attempt), 'ready')
  const partner = join(process.env.FAKE_BARRIER, (agent === 'claude' ? 'crabcode' : 'claude') + '-' + attempt)
  const deadline = Date.now() + 5000
  while (!existsSync(partner)) {
    if (Date.now() > deadline) throw new Error('agents did not overlap')
    await Bun.sleep(10)
  }
}
const file = existsSync('src/page.ts') ? 'src/page.ts' : 'stats.js'
if (!existsSync(file)) throw new Error('unknown fixture')
if (process.env.FAKE_MODE !== 'retry' || attempt > 1)
  writeFileSync(file, file === 'src/page.ts' ? ${JSON.stringify(solution)} : ${JSON.stringify(statsSolution)})
const index = args.indexOf('--trace-jsonl')
if (index !== -1) {
  const path = args[index + 1]
  if (agent !== 'crabcode' || index !== args.length - 3 || !isAbsolute(path) ||
      !path.endsWith('/attempt-' + attempt + '/trace.jsonl')) throw new Error('wrong trace path')
  if (process.env.FAKE_TRACE !== 'missing')
    writeFileSync(path, process.env.FAKE_TRACE === 'malformed' ? '{"schema_version":1' : ${JSON.stringify(trace)})
}
const assistant = { type: 'assistant', message: { id: 'msg-1', model: 'claude-haiku-4-5', content: [
  { type: 'thinking', thinking: '🦀' }, { type: 'tool_use', id: 'read-1', name: 'Read', input: {} }
] } }
console.log(JSON.stringify(assistant)); console.log(JSON.stringify(assistant))
console.log(JSON.stringify({ type: 'result', duration_api_ms: 42, is_error: false, total_cost_usd: 0.01,
  usage: { input_tokens: 7, output_tokens: 9, cache_read_input_tokens: 2, cache_creation_input_tokens: 3 } }))
console.error('fake ' + agent + ' attempt ' + attempt)
if (process.env.FAKE_MODE === 'exit7') process.exit(7)
`
const text = (path: string) => fs.readFileSync(path, 'utf8')
const json = (path: string): any => JSON.parse(text(path))
const out = (root: string) => join(root, 'report data', 'results.json')
const markdown = (root: string) => join(root, 'report data', 'benchmark.md')
function calls(root: string): any[] {
  const lines = text(join(root, 'invocations.jsonl')).trim().split('\n')
  return lines.map((line) => JSON.parse(line))
}

function offlineTest(name: string, body: (root: string) => void | Promise<void>) {
  const execute = async () => {
    const root = fs.mkdtempSync(join(tmpdir(), 'unified-runner-test-'))
    try {
      fs.writeFileSync(join(root, 'fake cli.ts'), fakeSource, { mode: 0o755 })
      await body(root)
    } finally {
      fs.rmSync(root, { recursive: true, force: true })
    }
  }
  test(name, execute, 25_000)
}
function environment(root: string, overrides: NodeJS.ProcessEnv = {}) {
  // Explicit undefined also removes inherited custom templates in runProcess's merged env.
  const env: NodeJS.ProcessEnv = { ...process.env, CI: '1', NO_COLOR: '1' }
  for (const key of Object.keys(env)) if (/^(BENCH_|FAKE_)/.test(key)) env[key] = undefined
  const fake = join(root, 'fake cli.ts')
  for (const agent of AVAILABLE_AGENTS) env[`BENCH_${agent.replaceAll('-', '_').toUpperCase()}_BIN`] = fake
  env.FAKE_INVOCATIONS = join(root, 'invocations.jsonl')
  env.FAKE_READY = join(root, 'ready')
  return { ...env, ...overrides }
}
function command(root: string, extra: string[] = []) {
  const flags = `--agents claude,crabcode --model test/model --claude-model claude-haiku-4-5
--claude-effort low --reasoning-effort low --tool-profile coding
--tasks ${page.id} --runs 1 --max-attempts 3 --timeout-ms 10000`.split(/\s+/)
  const paths = ['--dir', join(root, 'runs'), '--out', out(root), '--report', markdown(root)]
  return { bin: process.execPath, args: [runner, ...flags, ...paths, ...extra] }
}
function report(root: string) {
  const saved = json(out(root))
  expect(json(join(saved.runRoot, 'results.json'))).toEqual(saved)
  return saved
}
async function run(root: string, extra: string[] = [], env: NodeJS.ProcessEnv = {}, code = 0) {
  const proc = await runProcess(command(root, extra), REPO_ROOT, 15_000, environment(root, env))
  expect(proc.exitCode, proc.stdout + proc.stderr).toBe(code)
  expect(proc.timedOut || proc.interrupted).toBe(false)
  return report(root)
}
const tokens = (t: any) => [t.inputTokens, t.outputTokens, t.cacheReadTokens, t.cacheWriteTokens]
function artifacts(saved: any, result: any) {
  for (const attempt of result.attempts) {
    const label = `${result.task}-${result.agent}-${result.repetition}`
    const path = join(saved.logsRoot, label, `attempt-${attempt.number}`)
    for (const artifact of Object.values(attempt.artifacts) as string[]) {
      expect(isAbsolute(artifact) && artifact.startsWith(path + '/') && fs.existsSync(artifact)).toBe(true)
    }
    expect(json(join(path, 'checks.json'))).toEqual(attempt.checks)
    expect(text(attempt.artifacts.stdoutPath)).toContain('"type":"result"')
    expect(text(attempt.artifacts.stderrPath)).toContain(`fake ${result.agent} attempt ${attempt.number}`)
  }
}
function repaired(saved: any) {
  for (const result of saved.results) {
    expect(result).toMatchObject({ ok: true, firstAttemptOk: false, exitCode: 0, timedOut: false })
    expect(result.verifiedAtMs).toBeGreaterThan(0)
    expect(result.attempts.map((a: any) => a.number)).toEqual([1, 2])
    const [first, second] = result.attempts
    expect(first.checks.some((c: any) => !c.pass)).toBe(true)
    expect(second.checks.every((c: any) => c.pass)).toBe(true)
    expect(text(second.artifacts.promptPath)).toContain('Verification after attempt 1')
    artifacts(saved, result)
  }
}

offlineTest('paired repairs alternate agents and retain logs after workspace cleanup', async (root) => {
  const saved = await run(root, ['--runs', '2'], { FAKE_MODE: 'retry' })
  const order = saved.results.map((r: any) => `${r.agent}:${r.repetition}`)
  expect(order).toEqual('claude:1 crabcode:1 crabcode:2 claude:2'.split(' '))
  const invoked = calls(root).map((c) => c.agent)
  expect(invoked).toEqual('claude claude crabcode crabcode crabcode crabcode claude claude'.split(' '))
  repaired(saved)
  expect(fs.readdirSync(saved.workspacesRoot)).toEqual([])
  for (const result of saved.results) {
    expect(fs.existsSync(result.workspace)).toBe(false)
    expect(result.model).toBe(result.agent === 'claude' ? 'claude-haiku-4-5' : 'test/model')
    for (const attempt of result.attempts) {
      expect([attempt.artifacts.tracePath, attempt.artifacts.stepsPath]).toEqual([undefined, undefined])
      expect(text(attempt.artifacts.commandPath)).not.toContain('--trace-jsonl')
      if (result.agent === 'crabcode') {
        const t = attempt.telemetry
        expect(tokens(t)).toEqual([null, null, null, null])
        expect(t).toMatchObject({ modelTurns: null, toolCalls: null, reportedApiMs: null, breakdown: null })
      }
    }
  }
  expect(text(markdown(root))).toContain('0/2 | 2/2')
})

offlineTest('parallel repairs overlap in independent workspaces', async (root) => {
  const barrier = join(root, 'parallel barrier')
  fs.mkdirSync(barrier)
  const saved = await run(root, ['--parallel'], { FAKE_MODE: 'retry', FAKE_BARRIER: barrier })
  expect(saved.parallel).toBe(true)
  expect(saved.results).toHaveLength(2)
  expect(new Set(calls(root).map((c) => c.cwd)).size).toBe(2)
  expect(fs.readdirSync(barrier).sort()).toEqual(['claude-1', 'claude-2', 'crabcode-1', 'crabcode-2'])
  repaired(saved)
  expect(text(markdown(root))).toContain('timing comparisons are confounded')
})

offlineTest('common JS tasks pass; graded fixtures stay unique and opt-in', async (root) => {
  const defaults = selectTasks(TASKS)
  expect(defaults.some((task) => task.id === 'bugfix-js')).toBe(true)
  expect(new Set(TASKS.map((task) => task.id)).size).toBe(TASKS.length)
  for (const fixture of [...codingTasks, ...workflowTasks]) {
    expect(TASKS.find((task) => task.id === fixture.id)).toMatchObject({ defaultEnabled: false, grader: fixture })
    expect(defaults.some((task) => task.id === fixture.id)).toBe(false)
    expect(selectTasks(TASKS, fixture.id)).toHaveLength(1)
  }
  const saved = await run(root, ['--tasks', 'bugfix-js', '--keep'])
  expect(saved.results).toHaveLength(2)
  expect(calls(root).map((c) => c.agent)).toEqual(['claude', 'crabcode'])
  for (const result of saved.results) {
    expect(result).toMatchObject({ task: 'bugfix-js', ok: true, firstAttemptOk: true, passedChecks: 2, totalChecks: 2 })
    expect(text(join(result.workspace, 'stats.js'))).toBe(statsSolution)
    artifacts(saved, result)
  }
})

offlineTest('diagnostics measure telemetry and retain per-attempt traces/steps', async (root) => {
  const saved = await run(root, ['--diagnostics'], { FAKE_MODE: 'retry' })
  repaired(saved)
  expect(calls(root)).toHaveLength(4) // No --help/--version subprocess preflight.
  const paths = new Set<string>()
  for (const result of saved.results) {
    for (const attempt of result.attempts) {
      const { commandPath, tracePath, stepsPath } = attempt.artifacts
      expect(stepsPath).toBe(join(dirname(commandPath), 'steps.json'))
      paths.add(stepsPath)
      expect(text(markdown(root))).toContain(stepsPath)
      const t = attempt.telemetry
      expect(t).toMatchObject({ modelTurns: 1, toolCalls: 1, repeatedToolCalls: 0, apiError: false })
      if (result.agent === 'crabcode') {
        expect(tracePath).toBe(join(dirname(commandPath), 'trace.jsonl'))
        expect(text(commandPath)).toContain(`--trace-jsonl ${shellQuote(tracePath)}`)
        expect(text(tracePath)).toBe(trace)
        expect(json(stepsPath)).toHaveLength(1)
        expect(json(stepsPath)[0]).toMatchObject({ providerMs: 20, firstResponseMs: 10, toolMs: 10, toolCalls: 1 })
        expect(tokens(t)).toEqual([11, 13, 3, 0])
        expect(t.breakdown).toMatchObject({ startupMs: 10, providerMs: 20, toolMs: 10 })
      } else {
        expect(tracePath).toBeUndefined()
        expect(text(commandPath)).not.toContain('--trace-jsonl')
        expect(json(stepsPath)).toBeNull()
        expect(tokens(t)).toEqual([7, 9, 2, 3])
        expect(t).toMatchObject({ reasoningBytes: 4, reportedApiMs: 42, reportedCostUsd: 0.01, breakdown: null })
      }
    }
  }
  expect(paths.size).toBe(4)
})

offlineTest('missing/malformed traces fail passing source without retry', async (root) => {
  for (const mode of ['missing', 'malformed']) {
    const dir = join(root, mode)
    fs.mkdirSync(dir)
    fs.writeFileSync(join(dir, 'fake cli.ts'), fakeSource, { mode: 0o755 })
    const saved = await run(dir, ['--agents', 'crabcode', '--diagnostics'], { FAKE_TRACE: mode }, 1)
    expect(calls(dir)).toHaveLength(1)
    const result = saved.results[0],
      attempt = result.attempts[0]
    expect(result).toMatchObject({ ok: false, firstAttemptOk: false, verifiedAtMs: null, exitCode: 0 })
    expect(result.attempts).toHaveLength(1)
    expect(attempt.checks.every((c: any) => c.pass)).toBe(true)
    expect(attempt.process.error).toContain('Diagnostic trace missing or invalid')
    expect(json(attempt.artifacts.stepsPath)).toBeNull()
    expect(fs.existsSync(attempt.artifacts.tracePath)).toBe(mode === 'malformed')
    expect(fs.existsSync(attempt.artifacts.commandPath) && fs.existsSync(attempt.artifacts.sourceDiffPath)).toBe(true)
    expect(text(markdown(dir))).toContain('Diagnostic trace missing or invalid')
  }
})

offlineTest('exit 7 overrides passing source and stops retries', async (root) => {
  const saved = await run(root, [], { FAKE_MODE: 'exit7' }, 1)
  expect(saved.results).toHaveLength(2)
  expect(calls(root)).toHaveLength(2)
  for (const result of saved.results) {
    expect(result).toMatchObject({ ok: false, firstAttemptOk: false, verifiedAtMs: null, exitCode: 7, timedOut: false })
    expect(result.error).toContain('exit code 7')
    expect(result.attempts).toHaveLength(1)
    expect(result.attempts[0].checks.every((c: any) => c.pass)).toBe(true)
    artifacts(saved, result)
  }
})

offlineTest('SIGINT exits 130, preserves partial results and cleans workspaces', async (root) => {
  const cmd = command(root, ['--runs', '2'])
  const env = environment(root, { FAKE_MODE: 'wait' })
  const child = spawn(cmd.bin, cmd.args, { cwd: REPO_ROOT, env, stdio: 'ignore' })
  const exited = once(child, 'close')
  const deadline = setTimeout(() => child.kill('SIGKILL'), 8000)
  try {
    for (let i = 0; i < 250 && !fs.existsSync(join(root, 'ready')) && child.exitCode === null; i++) await Bun.sleep(20)
    expect(fs.existsSync(join(root, 'ready'))).toBe(true)
    expect(child.kill('SIGINT')).toBe(true)
    expect((await exited)[0]).toBe(130)
    const saved = report(root)
    expect(saved.stopped).toBe(true)
    expect(saved.results).toHaveLength(1)
    expect(calls(root)).toHaveLength(1)
    const result = saved.results[0],
      attempt = result.attempts[0]
    expect(result).toMatchObject({ agent: 'claude', ok: false, firstAttemptOk: false, verifiedAtMs: null })
    expect(result.attempts).toHaveLength(1)
    expect(attempt.process).toMatchObject({ interrupted: true, timedOut: false })
    expect(result.error).toContain('interrupted')
    expect(fs.existsSync(attempt.artifacts.stdoutPath) && fs.existsSync(attempt.artifacts.stderrPath)).toBe(true)
    expect(fs.readdirSync(saved.workspacesRoot)).toEqual([])
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM')
    await exited
    clearTimeout(deadline)
  }
})

offlineTest('clean-room grading rejects tampering and symlinks but accepts fixes', (root) => {
  writeFiles(root, page.files)
  expect(gradeTask(page, root).passed).toBe(false)
  writeFiles(root, {
    'tests/page.test.ts': 'throw new Error("must not run")',
    'bunfig.toml': 'preload = ["./tests/page.test.ts"]',
    'src/extra.test.ts': 'throw new Error("must not run")',
    'src/helper.ts': 'export const value = 1',
  })
  expect(candidateSources(page, root)['src/helper.ts']).toBe('export const value = 1')
  expect(candidateSources(page, root)['src/extra.test.ts']).toBeUndefined()
  expect(gradeTask(page, root).passed).toBe(false)
  writeFiles(root, { 'src/page.ts': solution })
  expect(gradeTask(page, root).passed).toBe(true)
  expect(sourceDiff(page, root)['src/page.ts']).toEqual({ before: page.files['src/page.ts'], after: solution })
  fs.rmSync(join(root, 'src'), { recursive: true })
  fs.mkdirSync(join(root, 'src'))
  const outside = join(root, 'outside')
  writeFiles(outside, { 'secret.ts': solution })
  fs.symlinkSync(join(outside, 'secret.ts'), join(root, 'src/page.ts'))
  fs.symlinkSync(outside, join(root, 'src/linked'))
  expect(candidateSources(page, root)).toEqual({})
  expect(gradeTask(page, root).passed).toBe(false)
})

offlineTest('process results cover ENOENT, timeout under 3s and abortion', async (root) => {
  expect((await runProcess({ bin: join(root, 'missing'), args: [] }, root, 100)).error).toContain('ENOENT')
  // A safety fuse bounds child lifetime even if process-group termination regresses.
  const code = 'setTimeout(() => process.exit(99), 4000); setInterval(() => {}, 100)'
  const command = { bin: process.execPath, args: ['-e', code] }
  const timed = await runProcess(command, root, 100)
  expect(timed.timedOut).toBe(true)
  expect(timed.elapsedMs).toBeLessThan(3000)
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 100)
  try {
    const aborted = await runProcess(command, root, 5000, {}, controller.signal)
    expect(aborted.interrupted).toBe(true)
    expect(aborted.timedOut).toBe(false)
  } finally {
    clearTimeout(timer)
  }
})
