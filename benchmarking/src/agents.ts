import { accessSync, constants, existsSync } from 'node:fs'
import { delimiter, join, resolve } from 'node:path'
import { AVAILABLE_AGENTS, REPO_ROOT } from './defaults.ts'
import { shellQuote } from './format.ts'
import type { AgentName, BenchmarkTask, Command, Grade, GradedFixture, ToolProfile } from './types.ts'

export const AGENT_LABELS: Record<AgentName, string> = {
  crabcode: '🦀 crabcode',
  opencode: '🔲 opencode',
  codex: '⚛️ codex',
  'grok-build': '⬛ grok-build',
  claude: '✳️ claude',
}

export function displayAgent(agent: AgentName) {
  return AGENT_LABELS[agent] ?? agent
}

/** Env override key: `BENCH_GROK_BUILD_CMD` for agent `grok-build`. */
export function agentEnvPrefix(agent: AgentName) {
  return `BENCH_${agent.replace(/-/g, '_').toUpperCase()}`
}

export type AgentCommandOptions = {
  claudeModel?: string
  claudeEffort?: string
  claudeTools?: string
  reasoningEffort?: string
  tracePath?: string
  toolProfile?: 'coding' | 'native'
}

export function commandFor(agent: AgentName, prompt: string, model: string, options: AgentCommandOptions = {}): string {
  const requestedModel =
    agent === 'claude' ? (options.claudeModel ?? (process.env.BENCH_CLAUDE_MODEL?.trim() || model)) : model
  const agentModel = modelForAgent(agent, requestedModel)
  const effort =
    agent === 'claude'
      ? (options.claudeEffort ?? (process.env.BENCH_CLAUDE_EFFORT?.trim() || 'low'))
      : (options.reasoningEffort ?? (process.env.BENCH_CRABCODE_REASONING?.trim() || 'medium'))
  const envName = `${agentEnvPrefix(agent)}_CMD`
  const override = process.env[envName]
  if (agent === 'crabcode' && options.tracePath && override && !override.includes('{trace}')) {
    throw new Error(
      'BENCH_CRABCODE_CMD must include {trace} (for example, --trace-jsonl {trace}) when diagnostics are enabled',
    )
  }
  if (!override && agent === 'claude') {
    const command = claudeCommand(agentModel, prompt, effort, options.toolProfile)
    command.bin = configuredBinary('claude') ?? 'claude'
    if (options.claudeTools !== undefined) {
      const index = command.args.indexOf('--tools')
      if (index >= 0) command.args[index + 1] = options.claudeTools
      else command.args.splice(command.args.indexOf('--allowedTools'), 0, '--tools', options.claudeTools)
    }
    return [command.bin, ...command.args].map(shellQuote).join(' ')
  }

  const template = override || defaultCommand(agent, options)
  const tokens = {
    repo: shellQuote(REPO_ROOT),
    model: shellQuote(agentModel),
    prompt: shellQuote(prompt),
    trace: options.tracePath ? shellQuote(options.tracePath) : '',
    effort: shellQuote(effort),
  }
  return template.replace(/\{(repo|model|prompt|trace|effort)\}/g, (_, key: keyof typeof tokens) => tokens[key])
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
 * - claude: strip one provider prefix and require a Claude model id or supported alias
 */
export function modelForAgent(agent: AgentName, modelRef: string) {
  if (agent === 'claude') {
    const model = modelRef.replace(/^[^/]+\//, '')
    if (!/^claude-[^/\s]+$/.test(model) && !['haiku', 'sonnet', 'opus', 'fable'].includes(model)) {
      throw new Error(
        `Unsupported Claude model: ${modelRef}. Set --claude-model (or BENCH_CLAUDE_MODEL), or use --model claude-...; no fallback model is selected`,
      )
    }
    return model
  }
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

function defaultCommand(agent: AgentName, options: AgentCommandOptions) {
  const configured = configuredBinary(agent)
  const binary = configured ? shellQuote(configured) : agent
  switch (agent) {
    case 'crabcode':
      return defaultCrabcodeCommand(options)
    case 'opencode':
      return `${binary} run --dangerously-skip-permissions -m {model} {prompt}`
    case 'codex':
      return `${binary} exec --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m {model} {prompt}`
    case 'grok-build':
      return defaultGrokBuildCommand()
    case 'claude':
      throw new Error('Claude default commands must use claudeCommand')
  }
}

function defaultCrabcodeCommand(options: AgentCommandOptions) {
  const profile = options.toolProfile === 'coding' ? ' --agent build' : ''
  const trace = options.tracePath ? ' --trace-jsonl {trace}' : ''
  const args = `-p -m {model} --reasoning-effort {effort}${profile} --no-session-persistence --dangerously-skip-permissions${trace} {prompt}`
  const configured = configuredBinary('crabcode')
  if (configured) {
    return `${shellQuote(configured)} ${args}`
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
  const configured = configuredBinary('grok-build')
  if (configured) {
    return `${shellQuote(configured)} ${args}`
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
    const candidate = resolve(REPO_ROOT, dir, name)
    try {
      accessSync(candidate, constants.X_OK)
      return candidate
    } catch {}
  }
  return null
}

function configuredBinary(agent: AgentName) {
  const binary = process.env[`${agentEnvPrefix(agent)}_BIN`]?.trim()
  return binary ? resolve(REPO_ROOT, binary) : undefined
}

export function assertAgentName(value: string): asserts value is AgentName {
  if (!AVAILABLE_AGENTS.includes(value as AgentName)) {
    throw new Error(`Unknown agent: ${value}. Expected one of ${AVAILABLE_AGENTS.join(', ')}`)
  }
}

export const CODING_TOOLS = ['read', 'write', 'edit', 'bash', 'glob', 'grep'] as const
const CLAUDE_CODING_TOOLS = ['Read', 'Write', 'Edit', 'Bash', 'Glob', 'Grep']

export function crabcodeFixtureConfig(model: string, toolProfile: ToolProfile = 'native') {
  return {
    model,
    ...(model.includes('/') ? { enabled_providers: [model.split('/')[0]] } : {}),
    ...(toolProfile === 'coding' ? { agent: { build: { tools: [...CODING_TOOLS] } } } : {}),
    mcp: { 'claude-design': { type: 'remote', url: 'http://127.0.0.1:3456/v1/design/mcp', enabled: false } },
  }
}

export const RULES = `This is a disposable coding benchmark. Work only in this workspace.
Implement the task, run the existing Bun tests, and stop when complete.
Do not install dependencies, use network services, edit harness configuration, or commit.
Keep public APIs and tests intact. Your final answer should briefly state the change and validation.`

export function claudeCommand(
  model: string,
  prompt: string,
  effort: string,
  toolProfile: ToolProfile = 'native',
): Command {
  return {
    bin: process.env.BENCH_CLAUDE_BIN || 'claude',
    args: [
      '-p',
      '--model',
      model,
      '--effort',
      effort,
      '--output-format',
      'stream-json',
      '--verbose',
      '--no-session-persistence',
      '--safe-mode',
      '--setting-sources',
      '',
      '--strict-mcp-config',
      '--mcp-config',
      '{"mcpServers":{}}',
      ...(toolProfile === 'coding' ? ['--tools', CLAUDE_CODING_TOOLS.join(',')] : []),
      '--allowedTools',
      'Read,Write,Edit,Bash',
      '--permission-mode',
      'dontAsk',
      prompt,
    ],
  }
}

export function repairPrompt(task: GradedFixture, grades: Grade[]): string {
  const feedback = grades.map((grade, index) => {
    const failures = grade.checks.filter((check) => !check.passed)
    return `Verification after attempt ${index + 1}:\n${failures
      .map((check) => `${check.name}:\n${check.output.slice(-6_000)}`)
      .join('\n')}`
  })
  return [
    RULES,
    task.prompt,
    ...feedback,
    grades.length ? 'Continue fixing the current files. This is a fresh conversation; previous edits remain.' : '',
  ]
    .filter(Boolean)
    .join('\n\n')
}
