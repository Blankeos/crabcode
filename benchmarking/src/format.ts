export function shellQuote(value: string) {
  if (!value) return "''"
  return `'${value.replaceAll("'", `'\\''`)}'`
}

export function sanitizePathPart(value: string) {
  return value.replace(/[^a-zA-Z0-9._-]+/g, '-')
}

export function tailText(value: string, maxChars = 2_000) {
  if (!value.trim()) return ''
  if (value.length <= maxChars) return value.trim()
  return `... truncated ...\n${value.slice(value.length - maxChars).trim()}`
}

export function formatDuration(ms: number) {
  if (ms < 1000) return `${Math.round(ms)}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

export function estimateTokens(text: string) {
  return Math.ceil(text.length / 4)
}

export function estimateCost(inputTokens: number, outputTokens: number, inputUsdPerMillion: number, outputUsdPerMillion: number) {
  return (inputTokens / 1_000_000) * inputUsdPerMillion + (outputTokens / 1_000_000) * outputUsdPerMillion
}

export function formatUsd(value: number) {
  if (!value) return '$0.0000'
  return `$${value.toFixed(4)}`
}

export function sum(values: number[]) {
  return values.reduce((total, value) => total + value, 0)
}

export function escapeMarkdownTable(value: string) {
  return value.replaceAll('|', '\\|').replaceAll('\n', '<br>')
}

