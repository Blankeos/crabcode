// Make-shift benchmark for comparing crabcode, opencode, and codex on tiny agent tasks.
// Run via: `bun run scripts/bench-agents.ts`

// @ts-nocheck

import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, extname, join, resolve, sep } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer } from 'node:http'

type AgentName = 'crabcode' | 'opencode' | 'codex'

type Task = {
  id: string
  title: string
  prompt: string
  files: Record<string, string>
  site?: {
    root: string
  }
  check: (cwd: string) => CheckResult[]
}

type CheckResult = {
  name: string
  pass: boolean
  detail?: string
}

type RunResult = {
  agent: AgentName
  task: string
  ok: boolean
  passedChecks: number
  totalChecks: number
  elapsedMs: number
  estimatedInputTokens: number
  estimatedOutputTokens: number
  estimatedCostUsd: number
  exitCode: number | null
  timedOut: boolean
  error?: string
  workspace?: string
  stdoutPath?: string
  stderrPath?: string
  commandPath?: string
  stdoutTail?: string
  stderrTail?: string
}

const REPO_ROOT = resolve(import.meta.dir, '..')
const DEFAULT_MODEL = 'openai/gpt-5.3-codex-spark'
const DEFAULT_TIMEOUT_MS = 45_000
const DEFAULT_RUNS = 1
const DEFAULT_INPUT_USD_PER_MTOK = 1.25
const DEFAULT_OUTPUT_USD_PER_MTOK = 10
const DEFAULT_BENCHMARK_DIR = join(REPO_ROOT, '.benchmarks')
const DEFAULT_REPORT_DIR = join(REPO_ROOT, 'benchmark-reports')
const DEFAULT_AGENTS: AgentName[] = ['crabcode', 'opencode', 'codex']
const AGENT_LABELS: Record<AgentName, string> = {
  crabcode: '🦀 crabcode',
  opencode: '🔲 opencode',
  codex: '⚛️ codex',
}
const activeChildren = new Set<any>()
const activeWorkspaces = new Set<string>()
let activeRunRoot: string | null = null
let shutdownRequested = false

process.once('SIGINT', () => requestShutdown('SIGINT'))
process.once('SIGTERM', () => requestShutdown('SIGTERM'))

const TASKS: Task[] = [
  {
    id: 'bugfix-js',
    title: 'Fix a small JavaScript bug',
    files: {
      'package.json': JSON.stringify({ type: 'module' }, null, 2) + '\n',
      'stats.js': `export function average(nums) {
  if (nums.length === 0) return 0
  return nums.reduce((sum, n) => sum + n, 0)
}
`,
    },
    prompt: `Fix stats.js. average([2, 4, 6]) should return 4, average([10]) should return 10, and average([]) should keep returning 0. Keep the change minimal.`,
    check: (cwd) => {
      const stats = readFileSync(join(cwd, 'stats.js'), 'utf8')
      const hasDivide = /\/\s*nums\.length/.test(stats)
      const stillHandlesEmpty = /length\s*={2,3}\s*0/.test(stats) && /return\s+0/.test(stats)
      return [
        { name: 'divides by length', pass: hasDivide },
        { name: 'keeps empty-array guard', pass: stillHandlesEmpty },
      ]
    },
  },
  {
    id: 'add-rust-test',
    title: 'Add a focused Rust test',
    files: {
      'src/lib.rs': `pub fn slugify(input: &str) -> String {
    input
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_basic_text() {
        assert_eq!(slugify("Hello Crab Code"), "hello-crab-code");
    }
}
`,
      'Cargo.toml': `[package]
name = "bench-fixture"
version = "0.0.0"
edition = "2021"
`,
    },
    prompt: `Add one focused test in src/lib.rs for slugify that covers leading/trailing whitespace and repeated internal whitespace. Do not change the slugify implementation.`,
    check: (cwd) => {
      const lib = readFileSync(join(cwd, 'src/lib.rs'), 'utf8')
      return [
        { name: 'adds a second test', pass: (lib.match(/#\[test\]/g) ?? []).length >= 2 },
        { name: 'covers whitespace case', pass: /\\t|\\n| {2,}|leading|trailing|whitespace/i.test(lib) },
        { name: 'does not change implementation shape', pass: lib.includes('.split_whitespace()') },
      ]
    },
  },
  {
    id: 'config-doc-sync',
    title: 'Synchronize tiny config docs',
    files: {
      'config.json': JSON.stringify(
        {
          model: DEFAULT_MODEL,
          agent: { build: { steps: 20 } },
        },
        null,
        2,
      ) + '\n',
      'README.md': `# Fixture

Default model: openai/gpt-5.3-codex-spark
Build steps: 12
`,
    },
    prompt: `Update README.md so the documented Build steps value matches config.json. Do not change config.json.`,
    check: (cwd) => {
      const config = readFileSync(join(cwd, 'config.json'), 'utf8')
      const readme = readFileSync(join(cwd, 'README.md'), 'utf8')
      return [
        { name: 'README documents 20 steps', pass: /Build steps:\s*20/.test(readme) },
        { name: 'config remains unchanged', pass: config.includes('"steps": 20') },
      ]
    },
  },
  {
    id: 'local-site-fetch',
    title: 'Fetch local site data and update docs',
    site: {
      root: 'site',
    },
    files: {
      'site/api/releases.json': JSON.stringify(
        {
          releases: [
            {
              version: '1.8.0-beta.1',
              channel: 'beta',
              recommended: false,
              migrationNote: 'Beta users should keep the experimental flag enabled.',
            },
            {
              version: '1.7.4',
              channel: 'stable',
              recommended: true,
              migrationNote: 'Set `snapshotMode` to `sparse` before rollout.',
            },
          ],
        },
        null,
        2,
      ) + '\n',
      'docs/release.md': `# Release Notes

Recommended stable: 1.6.2
Migration note: TBD
`,
    },
    prompt: `Fetch {siteUrl}/api/releases.json, find the recommended stable release, and update docs/release.md with its version and migrationNote. Do not change files under site/.`,
    check: (cwd) => {
      const doc = readFileSync(join(cwd, 'docs/release.md'), 'utf8')
      const siteData = readFileSync(join(cwd, 'site/api/releases.json'), 'utf8')
      return [
        { name: 'documents recommended stable version', pass: /1\.7\.4/.test(doc) },
        { name: 'copies fetched migration note', pass: /snapshotMode/.test(doc) && /sparse/.test(doc) },
        { name: 'removes placeholder note', pass: !/TBD/.test(doc) },
        { name: 'keeps served fixture intact', pass: siteData.includes('"version": "1.7.4"') },
      ]
    },
  },
  {
    id: 'invoice-ts-fix',
    title: 'Fix a cross-file TypeScript invoice bug',
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
  },
  {
    id: 'jsonc-config-parser',
    title: 'Add tiny JSONC config parser support',
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
    "model": "openai/gpt-5.3-codex-spark",
    "limits": {
      "maxTurns": 8,
    },
    "features": [
      "shell",
      "edit",
    ],
  }\`)

  expect(config).toEqual({
    model: 'openai/gpt-5.3-codex-spark',
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
  },
]

const args = parseArgs(process.argv.slice(2))
const agents = parseAgents(args.agents ?? process.env.BENCH_AGENTS ?? DEFAULT_AGENTS.join(','))
const selectedTasks = parseTasks(args.tasks ?? process.env.BENCH_TASKS)
const model = String(args.model ?? process.env.BENCH_MODEL ?? DEFAULT_MODEL)
const timeoutMs = Number(args['timeout-ms'] ?? process.env.BENCH_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
const runs = Number(args.runs ?? process.env.BENCH_RUNS ?? DEFAULT_RUNS)
const keep = Boolean(args.keep)
const estimateOnly = Boolean(args.estimate)
const inputPrice = Number(args['input-price'] ?? process.env.BENCH_INPUT_USD_PER_MTOK ?? DEFAULT_INPUT_USD_PER_MTOK)
const outputPrice = Number(args['output-price'] ?? process.env.BENCH_OUTPUT_USD_PER_MTOK ?? DEFAULT_OUTPUT_USD_PER_MTOK)
const outputPath = args.out ? resolve(String(args.out)) : null
const runId = timestampForPath()
const runRoot = createRunRoot(args.dir ?? process.env.BENCH_DIR, runId)
const workspacesRoot = join(runRoot, 'workspaces')
const logsRoot = join(runRoot, 'logs')
mkdirSync(workspacesRoot, { recursive: true })
mkdirSync(logsRoot, { recursive: true })
const reportPath = args['no-report']
  ? null
  : args.report && args.report !== true
    ? resolve(String(args.report))
    : join(resolve(String(args['report-dir'] ?? process.env.BENCH_REPORT_DIR ?? DEFAULT_REPORT_DIR)), `agent-benchmark-${runId}.md`)

if (args.help) {
  printHelp()
  process.exit(0)
}

if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
  throw new Error('--timeout-ms must be a positive number')
}

if (!Number.isFinite(runs) || runs <= 0) {
  throw new Error('--runs must be a positive number')
}

const plannedPrompts = selectedTasks.length * agents.length * runs
const estimatedInputTokens = selectedTasks.reduce((sum, task) => sum + estimateTokens(benchmarkPrompt(task.prompt)), 0) * agents.length * runs
const plannedCost = estimateCost(estimatedInputTokens, 0, inputPrice, outputPrice)

printIntro()

if (estimateOnly) {
  process.exit(0)
}

activeRunRoot = runRoot
printPaths()

const results: RunResult[] = []
writeCurrentMarkdownReport()

try {
  let runNumber = 0

  runLoop: for (let runIndex = 0; runIndex < runs; runIndex++) {
    for (const task of selectedTasks) {
      for (const agent of agents) {
        if (shutdownRequested) break runLoop
        runNumber += 1
        const result = await safeRunBenchmark(agent, task, runIndex, runNumber, plannedPrompts)
        results.push(result)
        writeCurrentMarkdownReport()
        printResult(result)
      }
    }
  }

  printSummary(results)

  if (reportPath) {
    writeCurrentMarkdownReport()
    console.log(`\nWrote Markdown report to ${reportPath}`)
  }

  if (shutdownRequested) {
    process.exitCode = 130
  }

  if (outputPath) {
    writeFileSync(
      outputPath,
      JSON.stringify(
        {
          generatedAt: new Date().toISOString(),
          model,
          agents,
          tasks: selectedTasks.map((task) => task.id),
          runs,
          runId,
          runRoot,
          workspacesRoot,
          logsRoot,
          markdownReport: reportPath,
          agentModels: Object.fromEntries(agents.map((agent) => [agent, modelForAgent(agent, model)])),
          pricing: {
            inputUsdPerMillionTokens: inputPrice,
            outputUsdPerMillionTokens: outputPrice,
          },
          results,
        },
        null,
        2,
      ) + '\n',
    )
    console.log(`\nWrote JSON results to ${outputPath}`)
  }
} finally {
  writeCurrentMarkdownReport()
  if (keep) {
    console.log(`\nKept benchmark workspaces in ${workspacesRoot}`)
  } else {
    cleanupWorkspaceChildren(workspacesRoot)
  }
  activeRunRoot = null
}

function writeCurrentMarkdownReport() {
  if (!reportPath || estimateOnly) return

  writeMarkdownReport(reportPath, {
    runId,
    runRoot,
    workspacesRoot,
    logsRoot,
    model,
    agents,
    tasks: selectedTasks,
    runs,
    plannedPrompts,
    timeoutMs,
    keep,
    inputPrice,
    outputPrice,
    results,
    stopped: shutdownRequested,
  })
}

async function safeRunBenchmark(
  agent: AgentName,
  task: Task,
  runIndex: number,
  runNumber: number,
  totalRuns: number,
): Promise<RunResult> {
  try {
    return await runBenchmark(agent, task, runIndex, runNumber, totalRuns)
  } catch (err) {
    return {
      agent,
      task: task.id,
      ok: false,
      passedChecks: 0,
      totalChecks: 0,
      elapsedMs: 0,
      estimatedInputTokens: 0,
      estimatedOutputTokens: 0,
      estimatedCostUsd: 0,
      exitCode: null,
      timedOut: false,
      error: `benchmark runner crashed: ${err instanceof Error ? err.message : String(err)}`,
    }
  }
}

async function runBenchmark(
  agent: AgentName,
  task: Task,
  runIndex: number,
  runNumber: number,
  totalRuns: number,
): Promise<RunResult> {
  const runLabel = `${String(runIndex + 1).padStart(2, '0')}-${agent}-${task.id}`
  const workspace = join(workspacesRoot, runLabel)
  mkdirSync(workspace, { recursive: true })
  activeWorkspaces.add(workspace)
  let staticServer: Awaited<ReturnType<typeof startStaticServer>> | null = null

  try {
    writeFixture(workspace, task)

    if (model) {
      writeFileSync(join(workspace, 'crabcode.jsonc'), JSON.stringify({ model }, null, 2) + '\n')
    }

    printRunStart(runNumber, totalRuns, agent, task.id, workspace)

    if (task.site) {
      try {
        staticServer = await startStaticServer(join(workspace, task.site.root))
      } catch (err) {
        const checks = runChecks(task, workspace)
        const passedChecks = checks.filter((check) => check.pass).length
        return {
          agent,
          task: task.id,
          ok: false,
          passedChecks,
          totalChecks: checks.length,
          elapsedMs: 0,
          estimatedInputTokens: 0,
          estimatedOutputTokens: 0,
          estimatedCostUsd: 0,
          exitCode: null,
          timedOut: false,
          error: `failed to start local static server: ${err instanceof Error ? err.message : String(err)}`,
          workspace,
        }
      }
    }

    const prompt = benchmarkPrompt(resolveTaskPrompt(task, staticServer?.url))
    const command = commandFor(agent, prompt)
    const started = performance.now()
    const proc = await runShell(command, workspace, timeoutMs)
    const elapsedMs = Math.round(performance.now() - started)
    const checks = runChecks(task, workspace)
    const passedChecks = checks.filter((check) => check.pass).length
    const output = `${proc.stdout}\n${proc.stderr}`.trim()
    const artifacts = writeRunArtifacts(runLabel, command, proc.stdout, proc.stderr)
    const estimatedInputTokens = estimateTokens(prompt)
    const estimatedOutputTokens = estimateTokens(output)
    const ok = !shutdownRequested && !proc.timedOut && proc.exitCode === 0 && passedChecks === checks.length
    const errors = [
      proc.timedOut ? `timed out after ${timeoutMs}ms` : '',
      proc.exitCode !== 0 && proc.exitCode !== null ? `exit code ${proc.exitCode}` : '',
      ...checks
        .filter((check) => !check.pass)
        .map((check) => `${check.name}${check.detail ? `: ${check.detail}` : ''}`),
      proc.error ?? '',
    ].filter(Boolean)

    return {
      agent,
      task: task.id,
      ok,
      passedChecks,
      totalChecks: checks.length,
      elapsedMs,
      estimatedInputTokens,
      estimatedOutputTokens,
      estimatedCostUsd: estimateCost(estimatedInputTokens, estimatedOutputTokens, inputPrice, outputPrice),
      exitCode: proc.exitCode,
      timedOut: proc.timedOut,
      error: errors.join('; ') || undefined,
      workspace,
      stdoutPath: artifacts.stdoutPath,
      stderrPath: artifacts.stderrPath,
      commandPath: artifacts.commandPath,
      stdoutTail: tailText(proc.stdout),
      stderrTail: tailText(proc.stderr),
    }
  } finally {
    await staticServer?.close()
    activeWorkspaces.delete(workspace)
  }
}

function createRunRoot(dir: string | boolean | undefined, runId: string) {
  const parent = dir && dir !== true ? resolve(String(dir)) : DEFAULT_BENCHMARK_DIR
  mkdirSync(parent, { recursive: true })
  const root = join(parent, runId)
  mkdirSync(root, { recursive: true })
  return root
}

function timestampForPath() {
  return new Date().toISOString().replaceAll(':', '').replaceAll('.', '-')
}

function commandFor(agent: AgentName, prompt: string) {
  const defaults: Record<AgentName, string> = {
    crabcode: defaultCrabcodeCommand(),
    opencode: 'opencode run --dangerously-skip-permissions -m {model} {prompt}',
    codex: 'codex exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}',
  }
  const envName = `BENCH_${agent.toUpperCase()}_CMD`
  const template = process.env[envName] || defaults[agent]
  const agentModel = modelForAgent(agent, model)
  return template
    .replaceAll('{repo}', shellQuote(REPO_ROOT))
    .replaceAll('{model}', shellQuote(agentModel))
    .replaceAll('{prompt}', shellQuote(prompt))
}

function benchmarkPrompt(prompt: string) {
  return [
    'You are running inside an isolated benchmark fixture.',
    'Modify files in the current working directory directly. Do not only describe the change.',
    'Keep the change minimal. When the task is complete, stop.',
    'If the task names exact file paths, inspect those paths directly instead of listing directories first.',
    'Do not repeat identical tool calls or run optional extra checks after the requested change is complete.',
    '',
    `Task: ${prompt}`,
  ].join('\n')
}

function resolveTaskPrompt(task: Task, siteUrl?: string) {
  return task.prompt.replaceAll('{siteUrl}', siteUrl ?? '')
}

function modelForAgent(agent: AgentName, modelRef: string) {
  if (agent === 'codex') {
    return modelRef.replace(/^openai\//, '')
  }

  return modelRef
}

function defaultCrabcodeCommand() {
  const binary = join(REPO_ROOT, 'target', 'debug', 'crabcode')
  if (existsSync(binary)) {
    return `${shellQuote(binary)} -p --no-session-persistence --dangerously-skip-permissions {prompt}`
  }
  return `cargo run --quiet --manifest-path ${shellQuote(join(REPO_ROOT, 'Cargo.toml'))} -- -p --no-session-persistence --dangerously-skip-permissions {prompt}`
}

function writeFixture(workspace: string, task: Task) {
  for (const [path, content] of Object.entries(task.files)) {
    const fullPath = join(workspace, path)
    mkdirSync(dirname(fullPath), { recursive: true })
    writeFileSync(fullPath, content)
  }
}

function runShell(command: string, cwd: string, timeoutMs: number) {
  return new Promise<{ stdout: string; stderr: string; exitCode: number | null; timedOut: boolean; error?: string }>(
    (resolveRun) => {
      const child = spawn(command, {
        cwd,
        shell: true,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: process.platform !== 'win32',
        env: {
          ...process.env,
          NO_COLOR: '1',
          CI: '1',
        },
      })

      let stdout = ''
      let stderr = ''
      let timedOut = false
      let settled = false
      activeChildren.add(child)

      const timer = setTimeout(() => {
        timedOut = true
        terminateChild(child, 'SIGTERM')
        setTimeout(() => terminateChild(child, 'SIGKILL'), 2_000).unref()
      }, timeoutMs)

      child.stdout.on('data', (chunk) => {
        stdout += chunk.toString()
      })
      child.stderr.on('data', (chunk) => {
        stderr += chunk.toString()
      })
      child.on('error', (err) => {
        if (settled) return
        settled = true
        activeChildren.delete(child)
        clearTimeout(timer)
        resolveRun({ stdout, stderr, exitCode: null, timedOut, error: err.message })
      })
      child.on('close', (code) => {
        if (settled) return
        settled = true
        activeChildren.delete(child)
        clearTimeout(timer)
        resolveRun({ stdout, stderr, exitCode: code, timedOut })
      })
    },
  )
}

function runChecks(task: Task, workspace: string): CheckResult[] {
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

function bunTestCheck(cwd: string): CheckResult {
  const result = runCheckCommand(cwd, process.execPath, ['test'])
  return {
    name: 'bun test passes',
    pass: result.ok,
    detail: result.detail,
  }
}

function runCheckCommand(cwd: string, command: string, args: string[]) {
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

async function startStaticServer(root: string) {
  const absoluteRoot = resolve(root)
  let lastError: Error | null = null

  for (let attempt = 0; attempt < 20; attempt++) {
    const port = 41_000 + Math.floor(Math.random() * 20_000)
    try {
      return await listenStaticServer(absoluteRoot, port)
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err))
    }
  }

  throw lastError ?? new Error('failed to start static server')
}

function listenStaticServer(absoluteRoot: string, port: number) {
  const server = createServer((request, response) => {
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      response.writeHead(405, { allow: 'GET, HEAD' })
      response.end('Method not allowed')
      return
    }

    let requestPath = 'index.html'
    try {
      const url = new URL(request.url ?? '/', 'http://127.0.0.1')
      requestPath = decodeURIComponent(url.pathname).replace(/^\/+/, '') || 'index.html'
    } catch {
      response.writeHead(400)
      response.end('Bad request')
      return
    }

    const filePath = resolve(absoluteRoot, requestPath)
    if (filePath !== absoluteRoot && !filePath.startsWith(absoluteRoot + sep)) {
      response.writeHead(403)
      response.end('Forbidden')
      return
    }

    if (!existsSync(filePath) || statSync(filePath).isDirectory()) {
      response.writeHead(404)
      response.end('Not found')
      return
    }

    response.writeHead(200, { 'content-type': contentTypeFor(filePath) })
    if (request.method === 'HEAD') {
      response.end()
      return
    }
    response.end(readFileSync(filePath))
  })

  return new Promise<{ url: string; close: () => Promise<void> }>((resolveStart, rejectStart) => {
    let settled = false
    const onError = (err: Error) => {
      if (settled) return
      settled = true
      rejectStart(err)
    }
    server.once('error', onError)
    try {
      server.listen(port, '127.0.0.1', () => {
        if (settled) return
        settled = true
        server.off('error', onError)
        resolveStart({
          url: `http://127.0.0.1:${port}`,
          close: () =>
            new Promise((resolveClose) => {
              server.close(() => resolveClose())
            }),
        })
      })
    } catch (err) {
      onError(err instanceof Error ? err : new Error(String(err)))
    }
  })
}

function contentTypeFor(path: string) {
  switch (extname(path)) {
    case '.json':
      return 'application/json; charset=utf-8'
    case '.md':
      return 'text/markdown; charset=utf-8'
    case '.txt':
      return 'text/plain; charset=utf-8'
    default:
      return 'application/octet-stream'
  }
}

function requestShutdown(signal: string) {
  if (shutdownRequested) {
    writeCurrentMarkdownReport()
    cleanupActiveWorkspaces()
    process.exit(signal === 'SIGINT' ? 130 : 143)
  }

  shutdownRequested = true
  console.error(`\nReceived ${signal}; stopping active agent processes...`)
  writeCurrentMarkdownReport()

  for (const child of activeChildren) {
    terminateChild(child, 'SIGTERM')
  }

  setTimeout(() => {
    for (const child of activeChildren) {
      terminateChild(child, 'SIGKILL')
    }
    writeCurrentMarkdownReport()
    cleanupActiveWorkspaces()
    process.exit(signal === 'SIGINT' ? 130 : 143)
  }, 2_500).unref()
}

function terminateChild(child: any, signal: NodeJS.Signals) {
  if (!child?.pid) return

  try {
    if (process.platform === 'win32') {
      spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' })
      return
    }

    process.kill(-child.pid, signal)
  } catch {
    try {
      child.kill(signal)
    } catch {}
  }
}

function cleanupActiveWorkspaces() {
  if (keep) return
  for (const workspace of activeWorkspaces) {
    cleanupWorkspace(workspace)
  }
  activeWorkspaces.clear()
  if (activeRunRoot) {
    cleanupWorkspace(activeRunRoot)
  }
}

function cleanupWorkspace(workspace: string) {
  try {
    rmSync(workspace, { recursive: true, force: true })
  } catch {}
}

function cleanupWorkspaceChildren(workspace: string) {
  try {
    for (const entry of readdirSync(workspace)) {
      cleanupWorkspace(join(workspace, entry))
    }
  } catch {}
}

function printIntro() {
  console.log('Agent benchmark')
  console.log('')
  console.log('Config')
  console.log(`  model:        ${model}`)
  console.log(`  agents:       ${agents.map(displayAgent).join(', ')}`)
  console.log(`  tasks:        ${selectedTasks.map((task) => task.id).join(', ')}`)
  console.log(`  runs:         ${runs}`)
  console.log(`  prompts:      ${plannedPrompts}`)
  console.log(`  timeout:      ${formatDuration(timeoutMs)}`)
  console.log(`  prompt cost:  ${formatUsd(plannedCost)} estimated`)
  console.log('')
  console.log('Agent model args')
  for (const agent of agents) {
    console.log(`  ${displayAgent(agent).padEnd(12)} ${modelForAgent(agent, model)}`)
  }
  console.log('')
}

function printPaths() {
  console.log('Paths')
  console.log(`  run:         ${runRoot}`)
  console.log(`  workspaces:  ${workspacesRoot}`)
  console.log(`  logs:        ${logsRoot}`)
  if (reportPath) {
    console.log(`  report:      ${reportPath}`)
  }
  console.log('')
  console.log('Notes')
  console.log('  Permission-gated actions are auto-approved for opencode and codex in isolated workspaces.')
  console.log('  Crabcode print mode is run with --dangerously-skip-permissions in isolated workspaces.')
  console.log('  Site-fetch tasks use a per-run 127.0.0.1 static server; they do not hit the public internet.')
  if (!keep) {
    console.log('  Workspaces are removed at exit. Pass --keep to preserve them.')
  }
  console.log('')
}

function printRunStart(runNumber: number, totalRuns: number, agent: AgentName, taskId: string, workspace: string) {
  console.log(`Run ${runNumber}/${totalRuns}: ${displayAgent(agent)} / ${taskId}`)
  console.log(`  workspace: ${workspace}`)
}

function printResult(result: RunResult) {
  const status = result.ok ? 'PASS' : 'FAIL'
  const checks = `${result.passedChecks}/${result.totalChecks}`
  console.log(`  result:    ${status}`)
  console.log(`  checks:    ${checks}`)
  console.log(`  time:      ${formatDuration(result.elapsedMs)}`)
  console.log(`  cost:      ${formatUsd(result.estimatedCostUsd)} estimated`)
  if (result.error) {
    console.log('  reason:')
    for (const line of result.error.split('; ')) {
      console.log(`    - ${line}`)
    }
  }
  if (result.stdoutPath || result.stderrPath) {
    console.log('  output:')
    if (result.stdoutPath) console.log(`    stdout: ${result.stdoutPath}`)
    if (result.stderrPath) console.log(`    stderr: ${result.stderrPath}`)
  }
  console.log('')
}

function printSummary(results: RunResult[]) {
  console.log('\nSummary')
  console.log('| Agent | Score | Checks | Avg time | Est. tokens | Est. cost |')
  console.log('|---|---:|---:|---:|---:|---:|')

  for (const row of summaryRows(results)) {
    console.log(`| ${displayAgent(row.agent)} | ${row.score} | ${row.checks} | ${row.avgTime} | ${row.tokens} | ${row.cost} |`)
  }

  console.log('\nMetric: Score is the percent of task runs where the command exited successfully and every deterministic check passed.')
  console.log('Cost is an estimate from prompt/output text tokens only; provider dashboards are the source of truth.')
}

function summaryRows(results: RunResult[]) {
  return agents.map((agent) => {
    const items = results.filter((result) => result.agent === agent)
    const passCount = items.filter((result) => result.ok).length
    const totalChecks = sum(items.map((item) => item.totalChecks))
    const passedChecks = sum(items.map((item) => item.passedChecks))
    const avgMs = items.length ? sum(items.map((item) => item.elapsedMs)) / items.length : 0
    const tokens = sum(items.map((item) => item.estimatedInputTokens + item.estimatedOutputTokens))
    const cost = sum(items.map((item) => item.estimatedCostUsd))
    return {
      agent,
      score: items.length ? `${Math.round((passCount / items.length) * 100)}%` : '0%',
      checks: `${passedChecks}/${totalChecks}`,
      avgTime: `${(avgMs / 1000).toFixed(1)}s`,
      tokens,
      cost: formatUsd(cost),
    }
  })
}

function writeMarkdownReport(
  path: string,
  report: {
    runId: string
    runRoot: string
    workspacesRoot: string
    logsRoot: string
    model: string
    agents: AgentName[]
    tasks: Task[]
    runs: number
    plannedPrompts: number
    timeoutMs: number
    keep: boolean
    inputPrice: number
    outputPrice: number
    results: RunResult[]
    stopped: boolean
  },
) {
  mkdirSync(dirname(path), { recursive: true })
  const lines: string[] = []

  lines.push(`# Agent Benchmark Report`)
  lines.push('')
  lines.push(`Generated: ${new Date().toISOString()}`)
  lines.push(`Run ID: \`${report.runId}\``)
  lines.push(`Model: \`${report.model || '(agent defaults)'}\``)
  lines.push(`Agent model args: ${report.agents.map((agent) => `\`${displayAgent(agent)}=${modelForAgent(agent, report.model)}\``).join(', ')}`)
  lines.push(`Agents: ${report.agents.map((agent) => `\`${displayAgent(agent)}\``).join(', ')}`)
  lines.push(`Tasks: ${report.tasks.map((task) => `\`${task.id}\``).join(', ')}`)
  lines.push(`Runs per agent/task: ${report.runs}`)
  lines.push(`Completed runs: ${report.results.length}/${report.plannedPrompts}`)
  lines.push(`Timeout per run: ${report.timeoutMs}ms`)
  lines.push(`Benchmark run directory: \`${report.runRoot}\``)
  lines.push(`Agents ran in: \`${report.workspacesRoot}\``)
  lines.push(`Logs: \`${report.logsRoot}\``)
  lines.push(`Workspaces kept after run: ${report.keep ? 'yes' : 'no'}`)
  lines.push(`Stopped early: ${report.stopped ? 'yes' : 'no'}`)
  lines.push('')
  lines.push(`Permission-gated actions are auto-approved for benchmark agent commands in isolated workspaces.`)
  lines.push(`Site-fetch tasks use a per-run 127.0.0.1 static server and do not hit the public internet.`)
  lines.push(`Cost is a rough estimate from prompt/output text tokens only; provider dashboards are the source of truth.`)
  lines.push('')

  lines.push(`## Summary`)
  lines.push('')
  lines.push('| Agent | Score | Checks | Avg time | Est. tokens | Est. cost |')
  lines.push('|---|---:|---:|---:|---:|---:|')
  for (const row of summaryRows(report.results)) {
    lines.push(`| ${displayAgent(row.agent)} | ${row.score} | ${row.checks} | ${row.avgTime} | ${row.tokens} | ${row.cost} |`)
  }
  lines.push('')

  lines.push(`## Runs`)
  lines.push('')
  lines.push('| Status | Agent | Task | Checks | Time | Est. tokens | Est. cost | Workspace | Stdout | Stderr | Error |')
  lines.push('|---|---|---|---:|---:|---:|---:|---|---|---|---|')
  for (const result of report.results) {
    const status = result.ok ? 'PASS' : 'FAIL'
    const tokens = result.estimatedInputTokens + result.estimatedOutputTokens
    lines.push(
      `| ${status} | ${displayAgent(result.agent)} | ${result.task} | ${result.passedChecks}/${result.totalChecks} | ${formatDuration(result.elapsedMs)} | ${tokens} | ${formatUsd(result.estimatedCostUsd)} | \`${result.workspace ?? ''}\` | \`${result.stdoutPath ?? ''}\` | \`${result.stderrPath ?? ''}\` | ${escapeMarkdownTable(result.error ?? '')} |`,
    )
  }
  lines.push('')

  lines.push(`## Output Tails`)
  lines.push('')
  for (const result of report.results) {
    if (!result.stdoutTail && !result.stderrTail) continue
    lines.push(`### ${displayAgent(result.agent)} / ${result.task}`)
    lines.push('')
    if (result.stdoutTail) {
      lines.push('stdout:')
      lines.push('```text')
      lines.push(result.stdoutTail)
      lines.push('```')
      lines.push('')
    }
    if (result.stderrTail) {
      lines.push('stderr:')
      lines.push('```text')
      lines.push(result.stderrTail)
      lines.push('```')
      lines.push('')
    }
  }

  lines.push(`## Tasks`)
  lines.push('')
  for (const task of report.tasks) {
    lines.push(`### ${task.id}`)
    lines.push('')
    lines.push(task.title)
    lines.push('')
    lines.push('```text')
    lines.push(task.prompt)
    lines.push('```')
    lines.push('')
  }

  writeFileSync(path, lines.join('\n') + '\n')
}

function escapeMarkdownTable(value: string) {
  return value.replaceAll('|', '\\|').replaceAll('\n', '<br>')
}

function displayAgent(agent: AgentName) {
  return AGENT_LABELS[agent] ?? agent
}

function writeRunArtifacts(runLabel: string, command: string, stdout: string, stderr: string) {
  const safeLabel = sanitizePathPart(runLabel)
  const commandPath = join(logsRoot, `${safeLabel}.command.txt`)
  const stdoutPath = join(logsRoot, `${safeLabel}.stdout.txt`)
  const stderrPath = join(logsRoot, `${safeLabel}.stderr.txt`)

  writeFileSync(commandPath, command + '\n')
  writeFileSync(stdoutPath, stdout)
  writeFileSync(stderrPath, stderr)

  return { commandPath, stdoutPath, stderrPath }
}

function sanitizePathPart(value: string) {
  return value.replace(/[^a-zA-Z0-9._-]+/g, '-')
}

function tailText(value: string, maxChars = 2_000) {
  if (!value.trim()) return ''
  if (value.length <= maxChars) return value.trim()
  return `... truncated ...\n${value.slice(value.length - maxChars).trim()}`
}

function formatDuration(ms: number) {
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

function estimateTokens(text: string) {
  return Math.ceil(text.length / 4)
}

function estimateCost(inputTokens: number, outputTokens: number, inputUsdPerMillion: number, outputUsdPerMillion: number) {
  return (inputTokens / 1_000_000) * inputUsdPerMillion + (outputTokens / 1_000_000) * outputUsdPerMillion
}

function formatUsd(value: number) {
  if (!value) return '$0.0000'
  return `$${value.toFixed(4)}`
}

function sum(values: number[]) {
  return values.reduce((total, value) => total + value, 0)
}

function parseArgs(raw: string[]) {
  const parsed: Record<string, string | boolean> = {}
  for (let i = 0; i < raw.length; i++) {
    const arg = raw[i]
    if (!arg.startsWith('--')) continue
    const body = arg.slice(2)
    const [key, inlineValue] = body.split('=', 2)
    if (inlineValue !== undefined) {
      parsed[key] = inlineValue
      continue
    }
    const next = raw[i + 1]
    if (next && !next.startsWith('--')) {
      parsed[key] = next
      i++
    } else {
      parsed[key] = true
    }
  }
  return parsed
}

function parseAgents(value: string): AgentName[] {
  const agents = value.split(',').map((agent) => agent.trim()).filter(Boolean)
  const valid = new Set(DEFAULT_AGENTS)
  for (const agent of agents) {
    if (!valid.has(agent as AgentName)) {
      throw new Error(`Unknown agent: ${agent}. Expected one of ${DEFAULT_AGENTS.join(', ')}`)
    }
  }
  return agents as AgentName[]
}

function parseTasks(value?: string | boolean): Task[] {
  if (!value || value === true) return TASKS
  const ids = String(value).split(',').map((task) => task.trim()).filter(Boolean)
  return ids.map((id) => {
    const task = TASKS.find((candidate) => candidate.id === id)
    if (!task) {
      throw new Error(`Unknown task: ${id}. Expected one of ${TASKS.map((task) => task.id).join(', ')}`)
    }
    return task
  })
}

function shellQuote(value: string) {
  if (!value) return "''"
  return `'${value.replaceAll("'", `'\\''`)}'`
}

function printHelp() {
  console.log(`Usage: bun run scripts/bench-agents.ts [options]

Options:
  --model provider/model             Model passed to each agent.
  --agents crabcode,opencode,codex   Agents to run.
  --tasks id-a,id-b                  Task IDs to run.
  --runs 1                           Repetitions per agent/task.
  --timeout-ms 45000                 Timeout per run.
  --estimate                         Print planned prompt count and prompt-only cost, then exit.
  --input-price 1.25                 Input USD per 1M tokens for rough cost estimates.
  --output-price 10                  Output USD per 1M tokens for rough cost estimates.
  --out bench-results.json           Write machine-readable JSON results.
  --report benchmark.md              Write Markdown report at an exact path.
  --report-dir benchmark-reports     Directory for default Markdown reports.
  --no-report                        Disable Markdown report generation.
  --dir .benchmarks                  Parent directory for benchmark runs.
  --keep                             Keep temporary workspaces for inspection.

Default params:
  model: ${DEFAULT_MODEL}
  agents: ${DEFAULT_AGENTS.join(',')}
  tasks: ${TASKS.map((task) => task.id).join(',')}
  runs: ${DEFAULT_RUNS}
  timeout-ms: ${DEFAULT_TIMEOUT_MS}
  input-price: ${DEFAULT_INPUT_USD_PER_MTOK}
  output-price: ${DEFAULT_OUTPUT_USD_PER_MTOK}
  dir: ${DEFAULT_BENCHMARK_DIR}
  report-dir: ${DEFAULT_REPORT_DIR}

Environment overrides:
  BENCH_MODEL, BENCH_AGENTS, BENCH_TASKS, BENCH_RUNS, BENCH_TIMEOUT_MS,
  BENCH_INPUT_USD_PER_MTOK, BENCH_OUTPUT_USD_PER_MTOK, BENCH_DIR, BENCH_REPORT_DIR

Stop behavior:
  Ctrl+C stops the active agent process tree and removes temporary workspaces unless --keep is set.

Command overrides:
  BENCH_CRABCODE_CMD='crabcode -p --no-session-persistence --dangerously-skip-permissions {prompt}'
  BENCH_OPENCODE_CMD='opencode run --dangerously-skip-permissions -m {model} {prompt}'
  BENCH_CODEX_CMD='codex exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}'

Template tokens: {prompt}, {model}, {repo}
Note: {model} is agent-aware; codex strips a leading openai/ provider prefix.
`)
}
