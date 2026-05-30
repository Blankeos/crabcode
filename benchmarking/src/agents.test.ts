import { afterEach, expect, test } from 'bun:test'
import { benchmarkPrompt, commandFor } from './agents.ts'

const originalCrabcodeCommand = process.env.BENCH_CRABCODE_CMD
const originalCrabcodeBin = process.env.BENCH_CRABCODE_BIN

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
})

test('default crabcode benchmark command pins the requested model', () => {
  delete process.env.BENCH_CRABCODE_CMD
  delete process.env.BENCH_CRABCODE_BIN

  const command = commandFor('crabcode', 'fix the fixture', 'openai/gpt-5.5')

  expect(command).toContain("-m 'openai/gpt-5.5'")
  expect(command).toContain("'fix the fixture'")
})

test('crabcode benchmark command supports an explicit optimized binary', () => {
  delete process.env.BENCH_CRABCODE_CMD
  process.env.BENCH_CRABCODE_BIN = '/tmp/crabcode-release'

  const command = commandFor('crabcode', 'fix the fixture', 'openai/gpt-5.5')

  expect(command).toContain("'/tmp/crabcode-release'")
  expect(command).toContain("-m 'openai/gpt-5.5'")
})

test('benchmark prompt asks agents to stop after concise validation summary', () => {
  const prompt = benchmarkPrompt('Fix the bug.')

  expect(prompt).toContain('When the task is complete, stop.')
  expect(prompt).toContain('After verification, give a final answer in at most two short lines')
  expect(prompt).toContain('Do not enumerate every edited file')
})
