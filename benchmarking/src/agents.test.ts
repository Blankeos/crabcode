import { afterEach, expect, test } from 'bun:test'
import { benchmarkPrompt, commandFor, modelForAgent } from './agents.ts'

const originalCrabcodeCommand = process.env.BENCH_CRABCODE_CMD
const originalCrabcodeBin = process.env.BENCH_CRABCODE_BIN
const originalCrabcodeReasoning = process.env.BENCH_CRABCODE_REASONING
const originalGrokBuildCommand = process.env.BENCH_GROK_BUILD_CMD
const originalGrokBuildBin = process.env.BENCH_GROK_BUILD_BIN

afterEach(() => {
  if (originalCrabcodeCommand === undefined) {
    delete process.env.BENCH_CRABCODE_CMD
  } else {
    process.env.BENCH_CRABCODE_CMD = originalCrabcodeCommand
  }
  if (originalCrabcodeBin === undefined) {
    delete process.env.BENCH_CRABCODE_BIN
  } else {
    process.env.BENCH_CRABCODE_BIN = originalCrabcodeBin
  }
  if (originalCrabcodeReasoning === undefined) {
    delete process.env.BENCH_CRABCODE_REASONING
  } else {
    process.env.BENCH_CRABCODE_REASONING = originalCrabcodeReasoning
  }
  if (originalGrokBuildCommand === undefined) {
    delete process.env.BENCH_GROK_BUILD_CMD
  } else {
    process.env.BENCH_GROK_BUILD_CMD = originalGrokBuildCommand
  }
  if (originalGrokBuildBin === undefined) {
    delete process.env.BENCH_GROK_BUILD_BIN
  } else {
    process.env.BENCH_GROK_BUILD_BIN = originalGrokBuildBin
  }
})

test('default crabcode benchmark command pins the requested model', () => {
  delete process.env.BENCH_CRABCODE_CMD
  delete process.env.BENCH_CRABCODE_BIN

  const command = commandFor('crabcode', 'fix the fixture', 'openai/gpt-5.5')

  expect(command).toContain("-m 'openai/gpt-5.5'")
  expect(command).toContain("--reasoning-effort 'medium'")
  expect(command).toContain("'fix the fixture'")
})

test('crabcode benchmark command supports an explicit optimized binary', () => {
  delete process.env.BENCH_CRABCODE_CMD
  process.env.BENCH_CRABCODE_BIN = '/tmp/crabcode-release'

  const command = commandFor('crabcode', 'fix the fixture', 'openai/gpt-5.5')

  expect(command).toContain("'/tmp/crabcode-release'")
  expect(command).toContain("-m 'openai/gpt-5.5'")
  expect(command).toContain("--reasoning-effort 'medium'")
})

test('crabcode benchmark command supports a reasoning override', () => {
  delete process.env.BENCH_CRABCODE_CMD
  delete process.env.BENCH_CRABCODE_BIN
  process.env.BENCH_CRABCODE_REASONING = 'low'

  const command = commandFor('crabcode', 'fix the fixture', 'openai/gpt-5.5')

  expect(command).toContain("--reasoning-effort 'low'")
})

test('benchmark prompt asks agents to stop after concise validation summary', () => {
  const prompt = benchmarkPrompt('Fix the bug.')

  expect(prompt).toContain('When the task is complete, stop.')
  expect(prompt).toContain('Do not invoke package managers or one-off formatter installs')
  expect(prompt).toContain('After verification, give a final answer in at most two short lines')
  expect(prompt).toContain('Do not enumerate every edited file')
})

test('grok-build command uses always-approve and single prompt', () => {
  delete process.env.BENCH_GROK_BUILD_CMD
  process.env.BENCH_GROK_BUILD_BIN = '/tmp/grok'

  const command = commandFor('grok-build', 'fix the fixture', 'grok-4.5')

  expect(command).toContain("'/tmp/grok'")
  expect(command).toContain('--always-approve')
  expect(command).toContain("-m 'grok-4.5'")
  expect(command).toContain("-p 'fix the fixture'")
})

test('grok-build strips provider prefix from model id', () => {
  expect(modelForAgent('grok-build', 'xai/grok-4.5')).toBe('grok-4.5')
  expect(modelForAgent('grok-build', 'grok-4.5')).toBe('grok-4.5')
})

test('grok-build env cmd override uses hyphen-safe env key', () => {
  process.env.BENCH_GROK_BUILD_CMD = 'custom-grok -m {model} -p {prompt}'
  const command = commandFor('grok-build', 'hi', 'grok-4.5')
  expect(command).toContain('custom-grok')
  expect(command).toContain("-m 'grok-4.5'")
})
