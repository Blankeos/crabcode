import { accessSync, constants, existsSync } from 'node:fs'
import { delimiter, join } from 'node:path'
import { DEFAULT_AGENTS, REPO_ROOT } from './defaults.ts'
import { shellQuote } from './format.ts'
import type { AgentName, BenchmarkTask } from './types.ts'

export const AGENT_LABELS: Record<AgentName, string> = {
  crabcode: '🦀 crabcode',
  opencode: '🔲 opencode',
  codex: '⚛️ codex',
  'grok-build': '⬛ grok-build',
}

export function displayAgent(agent: AgentName) {
  return AGENT_LABELS[agent] ?? agent
}

/** Env override key: `BENCH_GROK_BUILD_CMD` for agent `grok-build`. */
export function agentEnvPrefix(agent: AgentName) {
  return `BENCH_${agent.replace(/-/g, '_').toUpperCase()}`
}

export function commandFor(agent: AgentName, prompt: string, model: string) {
  const defaults: Record<AgentName, string> = {
    crabcode: defaultCrabcodeCommand(),
    opencode: 'opencode run --dangerously-skip-permissions -m {model} {prompt}',
    codex: 'codex exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}',
    'grok-build': defaultGrokBuildCommand(),
  }
  const envName = `${agentEnvPrefix(agent)}_CMD`
  const template = process.env[envName] || defaults[agent]
  const agentModel = modelForAgent(agent, model)
  return template
    .replaceAll('{repo}', shellQuote(REPO_ROOT))
    .replaceAll('{model}', shellQuote(agentModel))
    .replaceAll('{prompt}', shellQuote(prompt))
}

export function benchmarkPrompt(prompt: string) {
  return [
    'You are running inside an isolated benchmark fixture.',
    'Modify files in the current working directory directly. Do not only describe the change.',
    'Keep the change minimal. When the task is complete, stop.',
    'If the task names exact file paths, inspect those paths directly instead of listing directories first.',
    'Do not repeat identical tool calls or run optional extra checks after the requested change is complete.',
    'Do not invoke package managers or one-off formatter installs; use existing project scripts only.',
    'After verification, give a final answer in at most two short lines: what changed and what validation ran.',
    'Do not enumerate every edited file or continue explaining once the task is complete.',
    '',
    `Task: ${prompt}`,
  ].join('\n')
}

export function resolveTaskPrompt(task: BenchmarkTask, siteUrl?: string) {
  return task.prompt.replaceAll('{siteUrl}', siteUrl ?? '')
}

/**
 * Normalize model id for each harness CLI.
 * - codex: strip `openai/` prefix
 * - grok-build: strip a single `provider/` prefix when present (grok CLI takes bare ids);
 *   OpenAI-only ids will fail on grok — use a shared multi-provider model or omit grok for that run
 */
export function modelForAgent(agent: AgentName, modelRef: string) {
  if (agent === 'codex') {
    return modelRef.replace(/^openai\//, '')
  }
  if (agent === 'grok-build') {
    // `openai/gpt-5.5` → `gpt-5.5` (still may be unsupported); `grok-4.5` stays
    const stripped = modelRef.replace(/^[^/]+\//, '')
    return stripped || modelRef
  }

  return modelRef
}

function defaultCrabcodeCommand() {
  const reasoning = shellQuote(process.env.BENCH_CRABCODE_REASONING?.trim() || 'medium')
  const args = `-p -m {model} --reasoning-effort ${reasoning} --no-session-persistence --dangerously-skip-permissions {prompt}`
  const configuredBinary = process.env.BENCH_CRABCODE_BIN?.trim()
  if (configuredBinary) {
    return `${shellQuote(configuredBinary)} ${args}`
  }

  const installedBinary = findExecutableOnPath('crabcode')
  if (installedBinary) {
    return `${shellQuote(installedBinary)} ${args}`
  }

  const releaseBinary = join(REPO_ROOT, 'target', 'release', 'crabcode')
  if (existsSync(releaseBinary)) {
    return `${shellQuote(releaseBinary)} ${args}`
  }

  const binary = join(REPO_ROOT, 'target', 'debug', 'crabcode')
  if (existsSync(binary)) {
    return `${shellQuote(binary)} ${args}`
  }
  return `cargo run --quiet --manifest-path ${shellQuote(join(REPO_ROOT, 'Cargo.toml'))} -- ${args}`
}

/**
 * Grok Build headless: `--single` / `-p` runs one prompt and exits; `--always-approve`
 * auto-approves tools (bench workspace is disposable).
 * Override binary: BENCH_GROK_BUILD_BIN=/path/to/grok
 * Override full template: BENCH_GROK_BUILD_CMD='grok … -m {model} -p {prompt}'
 */
function defaultGrokBuildCommand() {
  const args = `--always-approve -m {model} -p {prompt}`
  const configuredBinary = process.env.BENCH_GROK_BUILD_BIN?.trim()
  if (configuredBinary) {
    return `${shellQuote(configuredBinary)} ${args}`
  }
  const installedBinary = findExecutableOnPath('grok')
  if (installedBinary) {
    return `${shellQuote(installedBinary)} ${args}`
  }
  // Fallback name if PATH has grok-build instead of grok
  const alt = findExecutableOnPath('grok-build')
  if (alt) {
    return `${shellQuote(alt)} ${args}`
  }
  return `grok ${args}`
}

function findExecutableOnPath(name: string) {
  const pathValue = process.env.PATH ?? ''
  for (const dir of pathValue.split(delimiter).filter(Boolean)) {
    const candidate = join(dir, name)
    try {
      accessSync(candidate, constants.X_OK)
      return candidate
    } catch {}
  }
  return null
}

export function assertAgentName(value: string): asserts value is AgentName {
  if (!DEFAULT_AGENTS.includes(value as AgentName)) {
    throw new Error(`Unknown agent: ${value}. Expected one of ${DEFAULT_AGENTS.join(', ')}`)
  }
}
