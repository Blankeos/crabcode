import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { DEFAULT_AGENTS, REPO_ROOT } from './defaults.ts'
import { shellQuote } from './format.ts'
import type { AgentName, BenchmarkTask } from './types.ts'

export const AGENT_LABELS: Record<AgentName, string> = {
  crabcode: '🦀 crabcode',
  opencode: '🔲 opencode',
  codex: '⚛️ codex',
}

export function displayAgent(agent: AgentName) {
  return AGENT_LABELS[agent] ?? agent
}

export function commandFor(agent: AgentName, prompt: string, model: string) {
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

export function benchmarkPrompt(prompt: string) {
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

export function resolveTaskPrompt(task: BenchmarkTask, siteUrl?: string) {
  return task.prompt.replaceAll('{siteUrl}', siteUrl ?? '')
}

export function modelForAgent(agent: AgentName, modelRef: string) {
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

export function assertAgentName(value: string): asserts value is AgentName {
  if (!DEFAULT_AGENTS.includes(value as AgentName)) {
    throw new Error(`Unknown agent: ${value}. Expected one of ${DEFAULT_AGENTS.join(', ')}`)
  }
}

