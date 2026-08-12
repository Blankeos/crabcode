import { join, resolve } from 'node:path'
import type { AgentName } from './types.ts'

export const REPO_ROOT = resolve(import.meta.dir, '..', '..')
export { DEFAULT_MODEL } from './models.ts'
export const DEFAULT_TIMEOUT_MS = 45_000
export const DEFAULT_RUNS = 1
export const DEFAULT_INPUT_USD_PER_MTOK = 1.25
export const DEFAULT_OUTPUT_USD_PER_MTOK = 10
export const DEFAULT_BENCHMARK_DIR = join(REPO_ROOT, '.benchmarks')
export const DEFAULT_REPORT_DIR = join(REPO_ROOT, 'benchmark-reports')
/** All known harnesses. Default runs include every agent (binary must be installed). */
export const DEFAULT_AGENTS: AgentName[] = ['crabcode', 'opencode', 'codex', 'grok-build']
