import { afterEach, beforeEach, expect, test } from 'bun:test'
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import {
  assertAgentName,
  benchmarkPrompt,
  commandFor,
  crabcodeFixtureConfig,
  repairPrompt,
  resolveTaskPrompt,
} from './agents.ts'
import { AVAILABLE_AGENTS, DEFAULT_AGENTS, REPO_ROOT } from './defaults.ts'
import { pairedOrder, shellQuote } from './format.ts'
import { codingTasks } from './tasks/coding.ts'
import { fixtureHash } from './workspace.ts'

const gptModel = 'openai/gpt-5.5'
const envKeys = [
  ...AVAILABLE_AGENTS.flatMap((agent) => {
    const prefix = `BENCH_${agent.replaceAll('-', '_').toUpperCase()}`
    return [`${prefix}_CMD`, `${prefix}_BIN`]
  }),
  'BENCH_CRABCODE_REASONING',
  'BENCH_CLAUDE_MODEL',
  'BENCH_CLAUDE_EFFORT',
  'PATH',
]
const originalEnv = Object.fromEntries(envKeys.map((key) => [key, process.env[key]]))
const temporaryDirectories: string[] = []

beforeEach(() => {
  for (const key of envKeys) {
    if (key !== 'PATH') delete process.env[key]
  }
})

afterEach(() => {
  for (const key of envKeys) {
    const value = originalEnv[key]
    if (value === undefined) delete process.env[key]
    else process.env[key] = value
  }
  for (const path of temporaryDirectories.splice(0)) rmSync(path, { recursive: true, force: true })
})

test('crabcode pins the model and supports binary, env effort, and explicit effort overrides', () => {
  const command = commandFor('crabcode', 'fix the fixture', gptModel)
  expect(command).toContain("-m 'openai/gpt-5.5'")
  expect(command).toContain("--reasoning-effort 'medium'")
  expect(command.endsWith("'fix the fixture'")).toBe(true)
  process.env.BENCH_CRABCODE_BIN = '/tmp/crabcode-release'
  process.env.BENCH_CRABCODE_REASONING = 'high'
  const configured = commandFor('crabcode', 'task', gptModel)
  expect(configured.startsWith("'/tmp/crabcode-release'")).toBe(true)
  expect(configured).toContain("--reasoning-effort 'high'")
  expect(commandFor('crabcode', 'task', gptModel, { reasoningEffort: 'low' })).toContain("--reasoning-effort 'low'")
})

test('grok-build strips the provider, approves a single prompt, and uses a hyphen-safe override key', () => {
  process.env.BENCH_GROK_BUILD_BIN = '/tmp/grok'
  expect(commandFor('grok-build', 'fix the fixture', 'xai/grok-4.5')).toBe(
    "'/tmp/grok' --always-approve -m 'grok-4.5' -p 'fix the fixture'",
  )
  process.env.BENCH_GROK_BUILD_CMD = 'custom-grok -m {model} -p {prompt}'
  expect(commandFor('grok-build', 'hi', 'grok-4.5')).toBe("custom-grok -m 'grok-4.5' -p 'hi'")
})

test('Claude is available only by opt-in', () => {
  expect(DEFAULT_AGENTS).toEqual(['crabcode', 'opencode', 'codex', 'grok-build'])
  expect(AVAILABLE_AGENTS).toEqual([...DEFAULT_AGENTS, 'claude'])
  expect(() => assertAgentName('claude')).not.toThrow()
  expect(() => assertAgentName('unknown')).toThrow('claude')
})

test('Claude rejects a global GPT model rather than falling back; its model and effort are independent', () => {
  expect(() => commandFor('claude', 'task', gptModel)).toThrow('Unsupported Claude model')
  process.env.BENCH_CLAUDE_MODEL = 'anthropic/claude-haiku-4-5'
  process.env.BENCH_CLAUDE_EFFORT = 'medium'
  const fromEnv = commandFor('claude', 'task', gptModel)
  expect(fromEnv).toContain("'--model' 'claude-haiku-4-5' '--effort' 'medium'")
  const options = { claudeModel: 'anthropic/sonnet', claudeEffort: 'high' }
  expect(commandFor('claude', 'task', gptModel, options)).toContain("'--model' 'sonnet' '--effort' 'high'")
  expect(commandFor('crabcode', 'task', gptModel, options)).toContain("-m 'openai/gpt-5.5'")
  process.env.BENCH_CLAUDE_CMD = 'custom-claude --model {model} --effort {effort} {prompt}'
  delete process.env.BENCH_CLAUDE_MODEL
  expect(() => commandFor('claude', 'task', gptModel)).toThrow('--claude-model')
  expect(commandFor('claude', 'task', gptModel, options)).toBe("custom-claude --model 'sonnet' --effort 'high' 'task'")
})

test('Claude uses isolated stream-json in native, coding, and custom-tool profiles', () => {
  const native = commandFor('claude', 'task', 'sonnet', { toolProfile: 'native' })
  expect(native).toContain("'--output-format' 'stream-json'")
  expect(native).toContain("'--verbose' '--no-session-persistence' '--safe-mode' '--setting-sources' ''")
  expect(native).toContain("'--strict-mcp-config' '--mcp-config' '{\"mcpServers\":{}}'")
  expect(native).not.toMatch(/--tools|--fallback-model|--resume/)
  const coding = commandFor('claude', 'task', 'sonnet', { toolProfile: 'coding' })
  const toolList = coding.match(/'--tools' '([^']+)'/)![1]
  const tools = toolList.toLowerCase().split(',')
  expect(tools).toEqual(['read', 'write', 'edit', 'bash', 'glob', 'grep'])
  expect(crabcodeFixtureConfig(gptModel, 'coding').agent?.build.tools).toEqual(tools)
  expect(crabcodeFixtureConfig(gptModel).agent).toBeUndefined()
  for (const toolProfile of ['coding', 'native'] as const) {
    const custom = commandFor('claude', 'task', 'sonnet', { toolProfile, claudeTools: 'Read,Bash' })
    expect(custom).toContain("'--tools' 'Read,Bash'")
    expect(custom).not.toContain('Read,Write,Edit,Bash,Glob,Grep')
    expect(custom).toContain("'--safe-mode'")
  }
})

test('commands shell-quote hostile-looking values and never recursively interpolate prompt tokens', () => {
  const prompt = "Fix 'x'; $(touch should-not-exist)\n{trace} {model} {repo} {effort}"
  const tools = "Read,Bash(echo 'ok'; $(touch should-not-exist))"
  const effort = "low'; echo nope"
  const claude = commandFor('claude', prompt, 'sonnet', { claudeTools: tools, claudeEffort: effort })
  expect(claude).toContain(`'--tools' ${shellQuote(tools)}`)
  expect(claude).toContain(`'--effort' ${shellQuote(effort)}`)
  expect(claude.endsWith(shellQuote(prompt))).toBe(true)
  process.env.BENCH_OPENCODE_CMD = 'custom --repo {repo} --model {model} --effort {effort} {trace} {prompt}'
  expect(commandFor('opencode', prompt, "provider/model's-id", { reasoningEffort: 'low' })).toBe(
    `custom --repo ${shellQuote(REPO_ROOT)} --model ${shellQuote("provider/model's-id")} --effort 'low'  ${shellQuote(prompt)}`,
  )
})

test('configured binaries resolve from the repo root; crabcode otherwise prefers PATH', () => {
  for (const agent of AVAILABLE_AGENTS) {
    const binary = `bin/${agent} custom's cli`
    process.env[`BENCH_${agent.replaceAll('-', '_').toUpperCase()}_BIN`] = binary
    const command = commandFor(agent, 'task', agent === 'claude' ? 'sonnet' : gptModel)
    expect(command.startsWith(shellQuote(resolve(REPO_ROOT, binary)))).toBe(true)
  }
  const root = mkdtempSync(join(tmpdir(), 'bench-agent-path-'))
  temporaryDirectories.push(root)
  const executable = join(root, 'crabcode')
  writeFileSync(executable, '#!/bin/sh\nexit 0\n')
  chmodSync(executable, 0o755)
  delete process.env.BENCH_CRABCODE_BIN
  process.env.PATH = root
  expect(commandFor('crabcode', 'task', gptModel).startsWith(shellQuote(executable))).toBe(true)
})

test('crabcode coding and trace flags precede the prompt, and overrides require {trace}', () => {
  const tracePath = "/tmp/artifacts' trace.jsonl"
  const options = { tracePath, toolProfile: 'coding' as const, reasoningEffort: 'low' }
  const traced = commandFor('crabcode', 'task', gptModel, options)
  expect(traced).toContain('--agent build')
  expect(traced.endsWith(`--trace-jsonl ${shellQuote(tracePath)} 'task'`)).toBe(true)
  expect(commandFor('crabcode', 'task', gptModel, { toolProfile: 'native' })).not.toMatch(/--agent|--trace-jsonl/)
  process.env.BENCH_CRABCODE_CMD = 'custom -m {model} {prompt}'
  expect(() => commandFor('crabcode', 'task', gptModel, options)).toThrow('BENCH_CRABCODE_CMD must include {trace}')
  process.env.BENCH_CRABCODE_CMD = 'custom --trace-jsonl {trace} --effort {effort} {prompt}'
  expect(commandFor('crabcode', 'task', gptModel, options)).toBe(
    `custom --trace-jsonl ${shellQuote(tracePath)} --effort 'low' 'task'`,
  )
})

test('prompts stay concise, interpolate site URLs, and retain the task with cumulative repair feedback', () => {
  expect(benchmarkPrompt('Fix the bug.')).toContain('at most two short lines')
  const site = { id: 'site', title: 'site', prompt: 'Fetch {siteUrl}', files: {}, check: () => [] }
  expect(resolveTaskPrompt(site, 'http://127.0.0.1:1234')).toBe('Fetch http://127.0.0.1:1234')
  expect(resolveTaskPrompt(site)).toBe('Fetch ')
  const grades = ['first failure', 'second failure'].map((output) => ({
    passed: false,
    checks: [{ name: 'hidden', passed: false, output }],
  }))
  const prompt = repairPrompt(codingTasks[0], grades)
  expect(prompt).toContain(codingTasks[0].prompt)
  expect(prompt).toMatch(/attempt 1:\nhidden:\nfirst failure[\s\S]*attempt 2:\nhidden:\nsecond failure/)
  expect(prompt).toContain('fresh conversation')
})

test('fixture hashes are stable but reflect prompt and file changes; pairing alternates', () => {
  const task = codingTasks[0]
  const hash = fixtureHash(task)
  expect(fixtureHash({ ...task, files: { ...task.files } })).toBe(hash)
  expect(fixtureHash({ ...task, prompt: 'different' })).not.toBe(hash)
  expect(fixtureHash({ ...task, files: { ...task.files, 'extra.ts': 'changed' } })).not.toBe(hash)
  expect(pairedOrder(0, ['claude', 'crabcode'])).toEqual(['claude', 'crabcode'])
  expect(pairedOrder(1, ['claude', 'crabcode'])).toEqual(['crabcode', 'claude'])
})
