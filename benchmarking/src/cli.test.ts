import { expect, test } from 'bun:test'
import { parseAgents, parseArgs, selectTasks } from './cli.ts'
import { DEFAULT_AGENTS } from './defaults.ts'
import type { BenchmarkTask } from './types.ts'

function task(id: string, overrides: Partial<BenchmarkTask> = {}): BenchmarkTask {
  return { id, title: id, prompt: 'Fix the fixture', files: {}, check: () => [], ...overrides }
}
const tasks = [
  task('smoke', { difficulty: 'smoke', tags: ['typescript'] }),
  task('hard', { difficulty: 'hard', tags: ['typescript', 'hidden-tests'], defaultEnabled: true }),
  task('optional', { difficulty: 'medium', tags: ['typescript', 'hidden-tests'], defaultEnabled: false }),
]
const ids = (selected: BenchmarkTask[]) => selected.map((task) => task.id)

test('Claude is opt-in; explicit agent lists reject duplicates, empty entries and unknown agents', () => {
  expect(DEFAULT_AGENTS).toEqual(['crabcode', 'opencode', 'codex', 'grok-build'])
  expect(parseAgents(' claude, crabcode ')).toEqual(['claude', 'crabcode'])
  expect(() => parseAgents('claude,claude')).toThrow('Duplicate agent')
  expect(() => parseAgents('crabcode,')).toThrow('non-empty comma-separated list')
  expect(() => parseAgents('missing')).toThrow('Unknown agent')
})

test('space and equals syntax preserve distinct global and Claude model, effort and tool options', () => {
  expect(
    parseArgs([
      '--model',
      'openai/gpt-5.5=version',
      '--claude-model=anthropic/claude-sonnet-5-5',
      '--claude-effort=low',
      '--claude-tools=Read,Write,Edit,Bash',
      '--reasoning-effort=medium',
      '--tool-profile',
      'coding',
      '--diagnostics',
    ]),
  ).toEqual({
    model: 'openai/gpt-5.5=version',
    'claude-model': 'anthropic/claude-sonnet-5-5',
    'claude-effort': 'low',
    'claude-tools': 'Read,Write,Edit,Bash',
    'reasoning-effort': 'medium',
    'tool-profile': 'coding',
    diagnostics: true,
  })
})

test('invalid flags fail rather than silently accepting missing values or boolean assignments', () => {
  for (const args of [['--model'], ['--model', '--keep'], ['--model='], ['--model', '  ']]) {
    expect(() => parseArgs(args)).toThrow('--model requires a value')
  }
  expect(() => parseArgs(['--claude-effort'])).toThrow('--claude-effort requires a value')
  expect(() => parseArgs(['--diagnostics=false'])).toThrow('does not take a value')
  expect(() => parseArgs(['--typo=value'])).toThrow('Unknown option')
  expect(() => parseArgs(['extra'])).toThrow('Unexpected argument')
})

test('only unfiltered defaults exclude opt-in tasks; IDs and filters can select them', () => {
  expect(ids(selectTasks(tasks))).toEqual(['smoke', 'hard'])
  expect(ids(selectTasks(tasks, 'optional, smoke'))).toEqual(['optional', 'smoke'])
  expect(ids(selectTasks(tasks, undefined, 'typescript,hidden-tests'))).toEqual(['hard', 'optional'])
  expect(ids(selectTasks(tasks, undefined, undefined, 'medium'))).toEqual(['optional'])
  expect(ids(selectTasks(tasks, 'optional,hard', 'hidden-tests', 'hard'))).toEqual(['hard'])
})

test('task selection rejects malformed filters, unknown IDs and empty matches', () => {
  expect(() => selectTasks(tasks, true)).toThrow('--tasks requires a value')
  expect(() => selectTasks(tasks, 'smoke,')).toThrow('non-empty comma-separated list')
  expect(() => selectTasks(tasks, undefined, '')).toThrow('--tags requires a value')
  expect(() => selectTasks(tasks, undefined, undefined, false)).toThrow('--difficulty requires a value')
  expect(() => selectTasks(tasks, 'missing')).toThrow('Unknown task')
  expect(() => selectTasks(tasks, 'optional', undefined, 'hard')).toThrow('No benchmark tasks matched')
})
