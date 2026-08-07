export type AgentName = 'crabcode' | 'opencode' | 'codex' | 'grok-build'

export type BenchmarkDifficulty = 'smoke' | 'medium' | 'hard'

export type BenchmarkTask = {
  id: string
  title: string
  prompt: string
  files: Record<string, string>
  /** When set, used unless `--model` / `BENCH_MODEL` overrides the whole run. */
  model?: string
  difficulty?: BenchmarkDifficulty
  tags?: string[]
  timeoutMs?: number
  site?: {
    root: string
  }
  check: (cwd: string) => CheckResult[]
}

export type CheckResult = {
  name: string
  pass: boolean
  detail?: string
}

export type RunResult = {
  agent: AgentName
  task: string
  model?: string
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

export type ParsedArgs = Record<string, string | boolean>

