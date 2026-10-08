import type { Telemetry } from './diagnostics.ts'

export type AgentName = 'crabcode' | 'opencode' | 'codex' | 'grok-build' | 'claude'

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
  /** Opt-in tasks are excluded only when no task IDs or filters are requested. */
  defaultEnabled?: boolean
  grader?: GradedFixture
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

export type BenchmarkAttempt = {
  number: number
  process: Omit<ProcessResult, 'stdout' | 'stderr'>
  checks: CheckResult[]
  telemetry?: Telemetry
  artifacts: {
    stdoutPath: string
    stderrPath: string
    commandPath: string
    tracePath?: string
    stepsPath?: string
    promptPath?: string
    sourceDiffPath?: string
  }
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
  repetition?: number
  firstAttemptOk?: boolean
  verifiedAtMs?: number | null
  attempts?: BenchmarkAttempt[]
  telemetry?: Telemetry
}

export type ParsedArgs = Record<string, string | boolean>

export type ProcessResult = {
  exitCode: number | null
  timedOut: boolean
  interrupted: boolean
  elapsedMs: number
  stdout: string
  stderr: string
  error?: string
}

export type Grade = { passed: boolean; checks: { name: string; passed: boolean; output: string }[] }

export type Command = { bin: string; args: string[] }
export type ToolProfile = 'coding' | 'native'
export type GradedFixture = {
  id: string
  title: string
  difficulty?: BenchmarkDifficulty
  prompt: string
  files: Record<string, string>
  checks: Record<string, string>
}
