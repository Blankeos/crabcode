import { createHash } from 'node:crypto'
import { median } from './format.ts'

export type Harness = 'claude' | 'crabcode'
export type DiagnosticStep = {
  step: number
  startedAtMs: number
  endedAtMs: number | null
  providerMs: number | null
  firstResponseMs: number | null
  firstResponseKind: string | null
  reason: string | null
  action: string | null
  messages: number | null
  tools: number | null
  messageTextBytes: number | null
  toolSchemaBytes: number | null
  reasoningBytes: number | null
  textBytes: number | null
  toolCalls: number
  toolMs: number | null
  retries: number
  retryDelayMs: number
  toolErrors: number
}
export type Breakdown = {
  startupMs: number | null
  providerMs: number | null
  toolMs: number | null
  medianFirstResponseMs: number | null
  retries: number
  requestedEffort: string | null
  sentEffort: string | null
  effortVerified: boolean
  toolErrors: number
  steps: DiagnosticStep[]
}
export type Telemetry = {
  toolCalls: number | null
  modelTurns: number | null
  reportedApiMs: number | null
  reasoningBytes: number | null
  toolSequence: { name: string; signature: string }[] | null
  repeatedToolCalls: number | null
  breakdown: Breakdown | null
  inputTokens: number | null
  outputTokens: number | null
  cacheReadTokens: number | null
  cacheWriteTokens: number | null
  reportedCostUsd: number | null
  models: string[]
  apiError: boolean
}

export function telemetry(harness: Harness, stdout: string, stderr: string, trace?: string): Telemetry {
  const result: Telemetry = {
    toolCalls: null,
    modelTurns: null,
    reportedApiMs: null,
    reasoningBytes: null,
    toolSequence: null,
    repeatedToolCalls: null,
    breakdown: null,
    inputTokens: null,
    outputTokens: null,
    cacheReadTokens: null,
    cacheWriteTokens: null,
    reportedCostUsd: null,
    models: [],
    apiError: false,
  }
  if (harness === 'crabcode') {
    // Prose is not evidence of tool calls, tokens, or an API error.
    if (trace !== undefined) traceTelemetry(result, trace)
    return result
  }
  let sawAssistant = false
  let missingMessageId = false
  const seen = new Set<string>()
  const messages = new Set<string>()
  const thinking = new Set<string>()
  const sequence: NonNullable<Telemetry['toolSequence']> = []
  let reasoningBytes = 0
  let hiddenThinking = false
  for (const event of jsonLines(stdout)) {
    if (event.type === 'system' && event.subtype === 'init' && event.model) result.models.push(event.model)
    if (event.type === 'assistant') {
      sawAssistant = true
      const messageId = event.message?.id
      if (typeof messageId === 'string') messages.add(messageId)
      else missingMessageId = true
      for (const block of Array.isArray(event.message?.content) ? event.message.content : []) {
        if (!block || typeof block !== 'object') continue
        if (
          block.type === 'redacted_thinking' ||
          (block.type === 'thinking' &&
            !block.thinking &&
            typeof block.signature === 'string' &&
            block.signature.length > 0)
        ) {
          hiddenThinking = true
        }
        if (block.type === 'thinking' && typeof block.thinking === 'string') {
          const key = JSON.stringify([messageId ?? event.uuid, block.thinking])
          if (!thinking.has(key)) {
            thinking.add(key)
            reasoningBytes += Buffer.byteLength(block.thinking)
          }
        }
        if (block.type !== 'tool_use' || typeof block.name !== 'string') continue
        const id = block.id || `${event.uuid}:${JSON.stringify(block)}`
        if (seen.has(id)) continue
        seen.add(id)
        sequence.push({ name: block.name, signature: toolSignature(block.name, block.input ?? null) })
      }
      if (event.message?.model) result.models.push(event.message.model)
    }
    if (event.type === 'result') {
      const usage = event.usage || {}
      result.inputTokens = usage.input_tokens ?? null
      result.outputTokens = usage.output_tokens ?? null
      result.cacheReadTokens = usage.cache_read_input_tokens ?? null
      result.cacheWriteTokens = usage.cache_creation_input_tokens ?? null
      result.reportedCostUsd = event.total_cost_usd ?? null
      result.reportedApiMs = nonnegative(event.duration_api_ms)
      result.apiError ||= Boolean(event.is_error)
      result.models.push(...Object.keys(event.modelUsage || {}))
    }
  }
  result.toolCalls = sawAssistant ? sequence.length : null
  result.modelTurns = sawAssistant && !missingMessageId ? messages.size : null
  result.reasoningBytes = sawAssistant && !hiddenThinking ? reasoningBytes : null
  result.toolSequence = sawAssistant ? sequence : null
  result.repeatedToolCalls = sawAssistant ? repeatedCalls(sequence) : null
  result.models = [...new Set(result.models)]
  return result
}

// Match Rust's name + NUL + recursively sorted JSON hashing. Signatures are
// comparable within a harness, not across different native tool vocabularies.
function toolSignature(name: string, input: unknown) {
  function canonical(value: any): string {
    if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`
    if (value && typeof value === 'object') {
      return `{${Object.keys(value)
        .sort()
        .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
        .join(',')}}`
    }
    return JSON.stringify(value)
  }
  return createHash('sha256').update(name).update('\0').update(canonical(input)).digest('hex')
}

function repeatedCalls(sequence: NonNullable<Telemetry['toolSequence']>) {
  return sequence.length - new Set(sequence.map(({ name, signature }) => JSON.stringify([name, signature]))).size
}

function jsonLines(text: string): any[] {
  return text.split('\n').flatMap((line) => {
    try {
      const event = JSON.parse(line)
      return event && typeof event === 'object' && !Array.isArray(event) ? [event] : []
    } catch {
      return []
    }
  })
}

function nonnegative(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

type Interval = [number, number]
function union(intervals: Interval[]): Interval[] {
  const merged: Interval[] = []
  for (const [start, end] of [...intervals].sort((a, b) => a[0] - b[0])) {
    const last = merged.at(-1)
    if (last && start <= last[1]) last[1] = Math.max(last[1], end)
    else merged.push([start, end])
  }
  return merged
}
function intervalMs(intervals: Interval[]) {
  return union(intervals).reduce((total, [start, end]) => total + end - start, 0)
}
function providerMs(intervals: Interval[], tools: Interval[]) {
  return union(intervals).reduce((total, [start, end]) => {
    const overlaps = tools.flatMap(([a, b]): Interval[] => {
      const interval: Interval = [Math.max(a, start), Math.min(b, end)]
      return interval[1] >= interval[0] ? [interval] : []
    })
    return total + end - start - intervalMs(overlaps)
  }, 0)
}

function traceTelemetry(result: Telemetry, trace: string) {
  const events = jsonLines(trace).filter(
    (event) => event.schema_version === 1 && typeof event.event === 'string' && nonnegative(event.elapsed_ms) !== null,
  )
  if (!events.length) return
  const breakdown: Breakdown = {
    startupMs: null,
    providerMs: null,
    toolMs: null,
    medianFirstResponseMs: null,
    retries: 0,
    requestedEffort: null,
    sentEffort: null,
    effortVerified: false,
    toolErrors: 0,
    steps: [],
  }
  result.breakdown = breakdown
  result.toolSequence = []
  const stepById = new Map<number, DiagnosticStep>()
  const closedSteps = new Set<DiagnosticStep>()
  let current: DiagnosticStep | undefined
  let sawModel = false
  const sentEfforts: (string | null)[] = []
  const retries = new Set<string>()
  const tools = new Map<string, { start: number; end: number | null; step: DiagnosticStep | undefined }>()
  const usages: any[] = []
  let metrics: any
  let completed = false
  let invalidToolInterval = false
  for (const event of events) {
    const at = event.elapsed_ms as number
    const step = event.step === undefined ? current : stepById.get(event.step)
    switch (event.event) {
      case 'model':
      case 'request_config':
        if (typeof event.model === 'string') result.models.push(event.model)
        if (event.event === 'model' && !sawModel) {
          sawModel = true
          breakdown.requestedEffort = event.requested_effort ?? null
        }
        if (event.event === 'request_config') {
          breakdown.sentEffort = event.sent_effort ?? null
          sentEfforts.push(breakdown.sentEffort)
        }
        break
      case 'provider_step_start':
        if (!Number.isSafeInteger(event.step) || event.step < 0) break
        if (current) closedSteps.add(current)
        current = {
          step: event.step,
          startedAtMs: at,
          endedAtMs: null,
          providerMs: null,
          firstResponseMs: null,
          firstResponseKind: null,
          reason: null,
          action: null,
          messages: nonnegative(event.messages),
          tools: nonnegative(event.tools),
          messageTextBytes: nonnegative(event.message_text_bytes),
          toolSchemaBytes: nonnegative(event.tool_schema_bytes),
          reasoningBytes: null,
          textBytes: null,
          toolCalls: 0,
          toolMs: null,
          retries: 0,
          retryDelayMs: 0,
          toolErrors: 0,
        }
        breakdown.startupMs ??= at
        breakdown.steps.push(current)
        stepById.set(event.step, current)
        break
      case 'first_response':
        if (step && step.firstResponseMs === null && at >= step.startedAtMs) {
          step.firstResponseMs = at - step.startedAtMs
          step.firstResponseKind = event.kind ?? null
        }
        break
      case 'provider_end':
        // Some transports emit a finish reason and response.completed for the
        // same step. Only the first closes the provider interval.
        if (step && step.endedAtMs === null && at >= step.startedAtMs) {
          step.endedAtMs = at
          step.reason = event.reason ?? null
        }
        break
      case 'step_finish':
        if (step) {
          step.action = event.action ?? null
          closedSteps.add(step)
        }
        break
      case 'tool_batch_end':
        if (step) closedSteps.add(step)
        break
      case 'response_summary':
        if (step) {
          // Each summary is a per-step snapshot, never a cumulative delta.
          step.reasoningBytes = nonnegative(event.reasoning_bytes)
          step.textBytes = nonnegative(event.text_bytes)
        }
        break
      case 'tool_start':
        if (typeof event.id !== 'string' || typeof event.name !== 'string' || typeof event.signature !== 'string') break
        if (tools.has(event.id)) break
        tools.set(event.id, { start: at, end: null, step })
        result.toolSequence.push({ name: event.name, signature: event.signature })
        if (step) step.toolCalls++
        break
      case 'tool_end': {
        const tool = tools.get(event.id)
        if (!tool || at < tool.start) invalidToolInterval = true
        else tool.end ??= at
        break
      }
      case 'retry':
      case 'provider_retry': {
        // The SDK retry metadata and host Retry chunk can describe the same
        // retry at slightly different timestamps. Count it once per step.
        const key = JSON.stringify([step?.step, event.attempt, event.delay_ms])
        if (retries.has(key)) break
        retries.add(key)
        breakdown.retries++
        if (step) {
          step.retries++
          step.retryDelayMs += nonnegative(event.delay_ms) ?? 0
        }
        break
      }
      case 'tool_error':
        breakdown.toolErrors++
        if (step) step.toolErrors++
        break
      case 'usage':
        usages.push(event)
        break
      case 'metrics':
        metrics = event // Summary: do not add this to usage deltas or API time.
        if (nonnegative(event.cost_usd) !== null) result.reportedCostUsd = event.cost_usd
        break
      case 'end':
        completed = true
        break
      case 'failed':
      case 'cancelled':
        result.apiError = true
        break
    }
  }
  const allTools = [...tools.values()]
  const toolIntervals: Interval[] = allTools.flatMap((tool) => (tool.end === null ? [] : [[tool.start, tool.end]]))
  const toolsComplete = !invalidToolInterval && allTools.every((tool) => tool.end !== null)
  const providerIntervals: Interval[] = []
  let providersComplete = breakdown.steps.length > 0
  for (const step of breakdown.steps) {
    if (step.endedAtMs !== null) {
      const interval: Interval = [step.startedAtMs, step.endedAtMs]
      providerIntervals.push(interval)
      if (!invalidToolInterval && !allTools.some((tool) => tool.end === null && tool.start < step.endedAtMs!)) {
        step.providerMs = providerMs([interval], toolIntervals)
      }
    }
    if (step.providerMs === null) providersComplete = false
    const stepTools = allTools.filter((tool) => tool.step === step)
    // A step can still be in flight even when its currently observed tools ended.
    if (!invalidToolInterval && (completed || closedSteps.has(step)) && stepTools.every((tool) => tool.end !== null)) {
      step.toolMs = intervalMs(stepTools.map((tool) => [tool.start, tool.end!] as Interval))
    }
  }
  if (completed) {
    if (toolsComplete) breakdown.toolMs = intervalMs(toolIntervals)
    if (providersComplete) breakdown.providerMs = providerMs(providerIntervals, toolIntervals)
  }
  breakdown.medianFirstResponseMs = median(
    breakdown.steps.flatMap((step) => (step.firstResponseMs === null ? [] : [step.firstResponseMs])),
  )
  breakdown.effortVerified =
    breakdown.requestedEffort !== null &&
    sentEfforts.length > 0 &&
    sentEfforts.every((sent) => sent === breakdown.requestedEffort)
  result.modelTurns = breakdown.steps.length
  result.toolCalls = tools.size
  result.repeatedToolCalls = repeatedCalls(result.toolSequence)
  if (breakdown.steps.length && breakdown.steps.every((step) => step.reasoningBytes !== null)) {
    result.reasoningBytes = breakdown.steps.reduce((sum, step) => sum + step.reasoningBytes!, 0)
  }
  const tokenFields = {
    inputTokens: 'input_tokens',
    outputTokens: 'output_tokens',
    cacheReadTokens: 'cache_read_tokens',
    cacheWriteTokens: 'cache_write_tokens',
  } as const
  const sources = usages.length ? usages : metrics ? [metrics] : []
  // No usage (including an all-zero placeholder) means unknown, not estimated 0.
  if (sources.some((source) => Object.values(tokenFields).some((field) => (nonnegative(source[field]) ?? 0) > 0))) {
    for (const [key, field] of Object.entries(tokenFields)) {
      const values = sources.flatMap((source) => (nonnegative(source[field]) === null ? [] : [source[field]]))
      result[key as keyof typeof tokenFields] = values.length ? values.reduce((sum, value) => sum + value, 0) : null
    }
  }
  result.models = [...new Set(result.models)]
}
