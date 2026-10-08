import { expect, test } from 'bun:test'
import { createHash } from 'node:crypto'
import { telemetry } from './diagnostics.ts'

function jsonl(events: Record<string, unknown>[]) {
  return events.map((event) => JSON.stringify(event)).join('\n') + '\n'
}

function traceJson(events: Record<string, unknown>[]) {
  return jsonl(events.map((event) => ({ schema_version: 1, ...event })))
}

test('Claude telemetry counts deduplicated tool calls and preserves reported usage without estimates', () => {
  const assistant = {
    type: 'assistant',
    message: {
      id: 'msg1',
      model: 'claude-haiku-4-5',
      content: [
        { type: 'tool_use', id: 'call-1', name: 'Read' },
        { type: 'tool_use', id: 'call-2', name: 'Edit' },
      ],
    },
  }
  const stdout = [
    { type: 'system', subtype: 'init', model: 'claude-haiku-4-5' },
    { type: 'assistant', message: { id: 'msg1', content: [{ type: 'thinking' }] } },
    assistant,
    assistant,
    {
      type: 'result',
      is_error: false,
      usage: { input_tokens: 12, output_tokens: 20, cache_read_input_tokens: 100, cache_creation_input_tokens: 50 },
      total_cost_usd: 0.02,
    },
  ]
    .map((event) => JSON.stringify(event))
    .join('\n')
  expect(telemetry('claude', stdout, '')).toEqual({
    toolCalls: 2,
    modelTurns: 1,
    reportedApiMs: null,
    reasoningBytes: 0,
    toolSequence: [
      { name: 'Read', signature: expect.any(String) },
      { name: 'Edit', signature: expect.any(String) },
    ],
    repeatedToolCalls: 0,
    breakdown: null,
    inputTokens: 12,
    outputTokens: 20,
    cacheReadTokens: 100,
    cacheWriteTokens: 50,
    reportedCostUsd: 0.02,
    models: ['claude-haiku-4-5'],
    apiError: false,
  })
  expect(telemetry('crabcode', 'done', 'I used some tools').toolCalls).toBeNull()
  expect(telemetry('claude', 'partial malformed JSON', '').inputTokens).toBeNull()
  expect(telemetry('claude', '{"type":"result","is_error":true}', '').apiError).toBe(true)
})

test('Claude split assistant blocks use distinct message IDs, UTF-8 thinking, and canonical tool inputs', () => {
  const thinking = { type: 'thinking', thinking: 'think🦀' }
  const input = { z: [{ y: 2, x: 1 }], a: { b: true, a: 'x' } }
  const reordered = { a: { a: 'x', b: true }, z: [{ x: 1, y: 2 }] }
  const first = { type: 'tool_use', id: 'call-1', name: 'Read', input }
  const assistant = (id: string, content: unknown[]) => ({ type: 'assistant', message: { id, content } })
  const parsed = telemetry(
    'claude',
    jsonl([
      assistant('msg-1', [thinking]),
      assistant('msg-1', [{ type: 'text', text: 'hi' }]),
      assistant('msg-1', [thinking, first]),
      assistant('msg-1', [first]),
      assistant('msg-2', [thinking, { ...first, id: 'call-2', input: reordered }]),
      assistant('msg-2', [{ ...first, id: 'call-3', name: 'Edit' }]),
      { type: 'result', num_turns: 99, duration_api_ms: 1234, usage: {} },
    ]),
    '',
  )
  expect(parsed.modelTurns).toBe(2)
  expect(parsed.reportedApiMs).toBe(1234)
  expect(parsed.reasoningBytes).toBe(2 * Buffer.byteLength('think🦀'))
  expect(parsed.toolCalls).toBe(3)
  expect(parsed.repeatedToolCalls).toBe(1)
  const signature = createHash('sha256').update('Read\0{"a":{"a":"x","b":true},"z":[{"x":1,"y":2}]}').digest('hex')
  expect(parsed.toolSequence?.map((call) => call.signature)).toEqual([signature, signature, expect.any(String)])
  expect(parsed.toolSequence?.[2].signature).not.toBe(signature)
  expect(parsed.breakdown).toBeNull()
  expect(parsed.inputTokens).toBeNull()
  expect(telemetry('claude', jsonl([{ type: 'result', num_turns: 4 }]), '').modelTurns).toBeNull()
  expect(telemetry('claude', jsonl([{ type: 'assistant', message: { content: [] } }]), '').modelTurns).toBeNull()
  expect(
    telemetry('claude', jsonl([{ type: 'assistant', message: { id: 'empty', content: [] } }]), '').toolSequence,
  ).toEqual([])
})

test('empty signed or redacted thinking is unavailable, not evidence of no reasoning', () => {
  for (const block of [
    { type: 'thinking', thinking: '', signature: 'private-signature' },
    { type: 'redacted_thinking', data: 'private-data' },
  ]) {
    const output = JSON.stringify({ type: 'assistant', message: { id: 'm1', content: [block] } })
    expect(telemetry('claude', output, '').reasoningBytes).toBeNull()
  }
})

test('trace timeline measures delayed/retried logical steps, first provider end, and parallel tool union', () => {
  const parsed = telemetry(
    'crabcode',
    'prose is ignored',
    '',
    traceJson([
      { event: 'init', elapsed_ms: 0 },
      {
        event: 'model',
        elapsed_ms: 2,
        provider: 'meridian',
        model: 'selected',
        requested_effort: 'low',
        sent_effort: 'low',
      },
      { event: 'request_config', elapsed_ms: 5, provider: 'meridian', model: 'resolved', sent_effort: null },
      {
        event: 'provider_step_start',
        elapsed_ms: 100,
        step: 1,
        messages: 2,
        tools: 6,
        message_text_bytes: 1000,
        tool_schema_bytes: 500,
      },
      { event: 'provider_retry', elapsed_ms: 149, step: 1, attempt: 1, delay_ms: 50 },
      { event: 'retry', elapsed_ms: 150, attempt: 1, delay_ms: 50 },
      { event: 'first_response', elapsed_ms: 350, step: 1, kind: 'reasoning' },
      { event: 'first_response', elapsed_ms: 400, step: 1, kind: 'tool_call' },
      { event: 'provider_end', elapsed_ms: 600, reason: 'tool_calls' },
      { event: 'provider_end', elapsed_ms: 620, reason: 'response_completed' },
      { event: 'response_summary', elapsed_ms: 625, step: 1, reasoning_bytes: 3, text_bytes: 0 },
      { event: 'response_summary', elapsed_ms: 630, step: 1, reasoning_bytes: 3, text_bytes: 0 },
      {
        event: 'usage',
        elapsed_ms: 640,
        input_tokens: 10,
        output_tokens: 4,
        cache_read_tokens: 0,
        cache_write_tokens: 2,
      },
      { event: 'tool_start', elapsed_ms: 650, id: 'a', name: 'read', signature: 'same', arguments_bytes: 12 },
      { event: 'tool_start', elapsed_ms: 700, id: 'b', name: 'read', signature: 'same', arguments_bytes: 12 },
      { event: 'tool_start', elapsed_ms: 710, id: 'b', name: 'read', signature: 'same', arguments_bytes: 12 },
      { event: 'tool_end', elapsed_ms: 800, id: 'a', name: 'read', output_bytes: 100 },
      { event: 'tool_error', elapsed_ms: 810, name: 'read' },
      { event: 'tool_end', elapsed_ms: 900, id: 'b', name: 'read', output_bytes: 20 },
      { event: 'tool_end', elapsed_ms: 910, id: 'b', name: 'read', output_bytes: 20 },
      { event: 'tool_start', elapsed_ms: 950, id: 'c', name: 'edit', signature: 'same', arguments_bytes: 12 },
      { event: 'tool_end', elapsed_ms: 1000, id: 'c', name: 'edit', output_bytes: 2 },
      { event: 'tool_batch_end', elapsed_ms: 1010, count: 3 },
      { event: 'step_finish', elapsed_ms: 1020, step: 1, action: 'continue' },
      { event: 'provider_step_start', elapsed_ms: 1100, step: 2, messages: 4, tools: 6 },
      { event: 'first_response', elapsed_ms: 1200, step: 2, kind: 'text' },
      { event: 'provider_end', elapsed_ms: 1500, reason: 'stop' },
      {
        event: 'usage',
        elapsed_ms: 1505,
        input_tokens: 20,
        output_tokens: 6,
        cache_read_tokens: 8,
        cache_write_tokens: 0,
      },
      { event: 'response_summary', elapsed_ms: 1510, step: 2, reasoning_bytes: 4, text_bytes: 20 },
      { event: 'step_finish', elapsed_ms: 1520, step: 2, action: 'finish' },
      {
        event: 'metrics',
        elapsed_ms: 1530,
        duration_ms: 1430,
        input_tokens: 30,
        output_tokens: 10,
        cache_read_tokens: 8,
        cache_write_tokens: 2,
      },
      { event: 'end', elapsed_ms: 1600 },
    ]),
  )
  expect(parsed).toMatchObject({
    models: ['selected', 'resolved'],
    modelTurns: 2,
    toolCalls: 3,
    repeatedToolCalls: 1,
    reasoningBytes: 7,
    reportedApiMs: null,
    inputTokens: 30,
    outputTokens: 10,
    cacheReadTokens: 8,
    cacheWriteTokens: 2,
    apiError: false,
    breakdown: {
      startupMs: 100,
      providerMs: 900,
      toolMs: 300,
      medianFirstResponseMs: 175,
      retries: 1,
      requestedEffort: 'low',
      sentEffort: null,
      effortVerified: false,
      toolErrors: 1,
    },
  })
  expect(parsed.breakdown?.steps).toHaveLength(2)
  expect(parsed.breakdown?.steps[0]).toMatchObject({
    step: 1,
    startedAtMs: 100,
    endedAtMs: 600,
    providerMs: 500,
    firstResponseMs: 250,
    firstResponseKind: 'reasoning',
    reason: 'tool_calls',
    action: 'continue',
    toolMs: 300,
    toolCalls: 3,
    reasoningBytes: 3,
    textBytes: 0,
    retries: 1,
    retryDelayMs: 50,
    toolErrors: 1,
    messageTextBytes: 1000,
    toolSchemaBytes: 500,
  })
  expect(parsed.breakdown?.steps[1]).toMatchObject({
    step: 2,
    providerMs: 400,
    toolMs: 0,
    firstResponseMs: 100,
    reasoningBytes: 4,
    toolCalls: 0,
  })
})

test('trace provider time excludes overlapping tool time instead of double counting', () => {
  const parsed = telemetry(
    'crabcode',
    '',
    '',
    traceJson([
      { event: 'provider_step_start', elapsed_ms: 100, step: 1 },
      { event: 'first_response', elapsed_ms: 110, step: 1, kind: 'tool_call' },
      { event: 'tool_start', elapsed_ms: 120, id: 'a', name: 'read', signature: 'a' },
      { event: 'tool_end', elapsed_ms: 180, id: 'a', name: 'read' },
      { event: 'provider_end', elapsed_ms: 200, reason: 'stop' },
      { event: 'step_finish', elapsed_ms: 210, step: 1, action: 'finish' },
      { event: 'end', elapsed_ms: 220 },
    ]),
  )
  expect(parsed.breakdown?.providerMs).toBe(40)
  expect(parsed.breakdown?.toolMs).toBe(60)
  expect(parsed.breakdown?.steps[0].providerMs).toBe(40)
})

test('tool batches and next steps close per-step tool time without requiring step_finish', () => {
  const parsed = telemetry(
    'crabcode',
    '',
    '',
    traceJson([
      { event: 'provider_step_start', elapsed_ms: 10, step: 1 },
      { event: 'provider_end', elapsed_ms: 20 },
      { event: 'tool_start', elapsed_ms: 30, id: 'a', name: 'read', signature: 'a' },
      { event: 'tool_end', elapsed_ms: 40, id: 'a' },
      { event: 'tool_batch_end', elapsed_ms: 45, count: 1 },
      { event: 'provider_step_start', elapsed_ms: 50, step: 2 },
      { event: 'provider_end', elapsed_ms: 60 },
      { event: 'tool_start', elapsed_ms: 70, id: 'b', name: 'read', signature: 'b' },
      { event: 'tool_end', elapsed_ms: 80, id: 'b' },
      { event: 'provider_step_start', elapsed_ms: 90, step: 3 },
    ]),
  )
  expect(parsed.breakdown?.steps.map((step) => step.toolMs)).toEqual([10, 10, null])
  expect(parsed.breakdown?.steps[0].action).toBeNull()
  expect(parsed.breakdown?.toolMs).toBeNull()
})

test('retry deduplication is scoped to a logical provider step', () => {
  const parsed = telemetry(
    'crabcode',
    '',
    '',
    traceJson([
      { event: 'provider_step_start', elapsed_ms: 0, step: 1 },
      { event: 'provider_retry', elapsed_ms: 1, step: 1, attempt: 1, delay_ms: 5 },
      { event: 'retry', elapsed_ms: 2, attempt: 1, delay_ms: 5 },
      { event: 'retry', elapsed_ms: 3, attempt: 2, delay_ms: 5 },
      { event: 'provider_end', elapsed_ms: 15 },
      { event: 'provider_step_start', elapsed_ms: 20, step: 2 },
      { event: 'retry', elapsed_ms: 21, attempt: 1, delay_ms: 5 },
      { event: 'provider_end', elapsed_ms: 30 },
      { event: 'end', elapsed_ms: 31 },
    ]),
  )
  expect(parsed.modelTurns).toBe(2)
  expect(parsed.breakdown?.retries).toBe(3)
  expect(parsed.breakdown?.steps.map((step) => [step.retries, step.retryDelayMs])).toEqual([
    [2, 10],
    [1, 5],
  ])
})

test('trace durations stay unknown without end or with unfinished provider/tools; failures are API errors', () => {
  const start = [
    { event: 'init', elapsed_ms: 0 },
    { event: 'provider_step_start', elapsed_ms: 100, step: 1 },
    { event: 'first_response', elapsed_ms: 200, step: 1, kind: 'text' },
  ]
  const partial = telemetry(
    'crabcode',
    '',
    '',
    traceJson([
      ...start,
      { event: 'provider_end', elapsed_ms: 500, reason: 'stop' },
      { event: 'step_finish', elapsed_ms: 510, step: 1, action: 'finish' },
      { event: 'metrics', elapsed_ms: 520, duration_ms: 420 },
    ]) + '{"schema_version":1',
  )
  expect(partial.breakdown).toMatchObject({
    startupMs: 100,
    providerMs: null,
    toolMs: null,
    medianFirstResponseMs: 100,
  })
  expect(partial.breakdown?.steps[0].providerMs).toBe(400)
  expect(partial.reportedApiMs).toBeNull()
  expect(partial.reasoningBytes).toBeNull()
  for (const failure of ['failed', 'cancelled']) {
    const failed = telemetry('crabcode', '', '', traceJson([...start, { event: failure, elapsed_ms: 300 }]))
    expect(failed.apiError).toBe(true)
    expect(failed.breakdown?.providerMs).toBeNull()
    expect(failed.breakdown?.toolMs).toBeNull()
    expect(failed.breakdown?.steps[0].providerMs).toBeNull()
  }
  const unfinished = telemetry(
    'crabcode',
    '',
    '',
    traceJson([
      ...start,
      { event: 'provider_end', elapsed_ms: 500 },
      { event: 'tool_start', elapsed_ms: 600, id: 'unfinished', name: 'read', signature: 'a' },
      { event: 'tool_batch_end', elapsed_ms: 700, count: 1 },
      { event: 'end', elapsed_ms: 800 },
    ]),
  )
  expect(unfinished.breakdown?.providerMs).toBe(400)
  expect(unfinished.breakdown?.toolMs).toBeNull()
  expect(unfinished.breakdown?.steps[0].toolMs).toBeNull()
  expect(
    telemetry('crabcode', '', '', traceJson([...start, { event: 'end', elapsed_ms: 300 }])).breakdown?.providerMs,
  ).toBeNull()
})

test('trace usage deltas win over metrics summaries; missing and all-zero usage is null', () => {
  const parse = (events: Record<string, unknown>[]) =>
    telemetry('crabcode', '', '', traceJson(events.map((event, index) => ({ elapsed_ms: index, ...event }))))
  expect(parse([{ event: 'init' }, { event: 'end' }]).inputTokens).toBeNull()
  const summary = {
    event: 'metrics',
    duration_ms: 100,
    input_tokens: 7,
    output_tokens: 9,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
  }
  const fallback = parse([summary, summary, { event: 'end' }])
  expect(fallback).toMatchObject({
    inputTokens: 7,
    outputTokens: 9,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    reportedApiMs: null,
  })
  const zero = parse([
    { event: 'usage', input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 },
    summary,
  ])
  expect(zero.inputTokens).toBeNull()
  expect(zero.outputTokens).toBeNull()
  expect(zero.cacheReadTokens).toBeNull()
  expect(zero.cacheWriteTokens).toBeNull()
  const missing = parse([{ event: 'usage', input_tokens: 2 }, { event: 'usage', input_tokens: 3 }, summary])
  expect(missing.inputTokens).toBe(5)
  expect(missing.outputTokens).toBeNull()
  expect(telemetry('crabcode', '', '', '{"schema_version":2,"event":"init","elapsed_ms":0}').breakdown).toBeNull()
})

test('effort verification uses initial requested effort and actual request_config, not model guesses', () => {
  const model = { event: 'model', elapsed_ms: 0, model: 'haiku', requested_effort: 'low', sent_effort: 'low' }
  const parse = (...events: Record<string, unknown>[]) =>
    telemetry('crabcode', '', '', traceJson([model, ...events])).breakdown!
  expect(parse().effortVerified).toBe(false)
  expect(parse({ event: 'request_config', elapsed_ms: 1, sent_effort: null })).toMatchObject({
    sentEffort: null,
    effortVerified: false,
  })
  expect(parse({ event: 'request_config', elapsed_ms: 1, sent_effort: 'high' })).toMatchObject({
    sentEffort: 'high',
    effortVerified: false,
  })
  expect(parse({ event: 'request_config', elapsed_ms: 1, sent_effort: 'low' })).toMatchObject({
    requestedEffort: 'low',
    sentEffort: 'low',
    effortVerified: true,
  })
  expect(
    parse(
      { event: 'request_config', elapsed_ms: 1, sent_effort: null },
      { event: 'request_config', elapsed_ms: 2, sent_effort: 'low' },
      { event: 'model', elapsed_ms: 3, requested_effort: 'high' },
    ),
  ).toMatchObject({ requestedEffort: 'low', sentEffort: 'low', effortVerified: false })
  expect(
    telemetry(
      'crabcode',
      '',
      '',
      traceJson([
        { event: 'model', elapsed_ms: 0, requested_effort: null },
        { event: 'request_config', elapsed_ms: 1, sent_effort: null },
      ]),
    ).breakdown?.effortVerified,
  ).toBe(false)
})
