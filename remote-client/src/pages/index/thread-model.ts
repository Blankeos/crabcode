import type { RemoteMessage, RemoteMessagePart, RemoteStatus } from "../../remote-api"
import type { ActionDescriptor, DiffLine, DiffSection, JsonObject, JsonValue, ParsedToolMessage, ThreadItem, ToolActivityStep, ToolMessage, ToolStepDetail, ToolVisualState } from "./page-types"
import { basename, cuid, formatSeconds } from "./shared-utils"

const THINKING_TOOL_NAMES = new Set([
  "glob",
  "grep",
  "list",
  "question",
  "read",
  "skill",
  "task",
  "todowrite",
  "update_plan",
  "view_image",
  "webfetch",
])
const EXPLORATION_TOOL_NAMES = new Set(["glob", "grep", "list", "read"])
const ACTION_TOOL_NAMES = new Set(["apply_patch", "bash", "edit", "write"])

export function buildThreadItems(messages: RemoteMessage[], cwd: string): ThreadItem[] {
  const items: ThreadItem[] = []
  let activeAssistantItem: Extract<ThreadItem, { type: "message" }> | null = null
  let orphanActivityTools: ToolMessage[] = []

  const flushOrphanActivity = () => {
    if (orphanActivityTools.length === 0) return
    items.push({ type: "activity", tools: orphanActivityTools })
    orphanActivityTools = []
  }

  for (const message of messages) {
    if (message.role !== "tool") {
      if (message.role === "assistant" && activeAssistantItem) {
        activeAssistantItem.message = mergeAssistantTurnMessages(
          activeAssistantItem.message,
          message
        )
        continue
      }

      flushOrphanActivity()
      const item: ThreadItem = { type: "message", message, activityTools: [] }
      items.push(item)
      activeAssistantItem = message.role === "assistant" ? item : null
      if (activeAssistantItem) {
        for (const tool of assistantPartToolMessages(message, cwd)) {
          if (isActivityTool(tool)) {
            activeAssistantItem.activityTools.push(tool)
          } else {
            flushOrphanActivity()
            items.push({ type: "action", tool })
          }
        }
      }
      continue
    }

    const tool = parseToolMessage(message, cwd)
    if (isActivityTool(tool)) {
      if (activeAssistantItem) {
        activeAssistantItem.activityTools.push(tool)
      } else {
        orphanActivityTools.push(tool)
      }
    } else {
      flushOrphanActivity()
      items.push({ type: "action", tool })
    }
  }

  flushOrphanActivity()
  return items
}

export function assistantPartToolMessages(message: RemoteMessage, cwd: string): ToolMessage[] {
  const parts = Array.isArray(message.parts) ? message.parts : []
  if (parts.length === 0) return []

  const resultIds = new Set(
    parts
      .filter((part) => part.type === "tool_result")
      .map(toolPartId)
      .filter((id): id is string => Boolean(id))
  )
  const callsById = new Map<string, RemoteMessagePart>()
  const tools: ToolMessage[] = []

  for (const part of parts) {
    if (part.type === "tool_call") {
      const id = toolPartId(part)
      if (id) callsById.set(id, part)
      if (!id || resultIds.has(id)) continue
      const payload = { ...toolPartPayload(part), status: stringValue(part.status) || "running" }
      tools.push(toolMessageFromPayload(message, payload, cwd))
      continue
    }

    if (part.type === "tool_result") {
      const id = toolPartId(part)
      const call = id ? callsById.get(id) : undefined
      const payload = toolPartPayload(part)
      if (payload.args === undefined && call?.args !== undefined) payload.args = call.args
      tools.push(toolMessageFromPayload(message, payload, cwd))
    }
  }

  return tools
}

export function toolPartId(part: RemoteMessagePart): string | null {
  return stringValue(part.id) || stringValue(part.call_id) || null
}

export function toolPartPayload(part: RemoteMessagePart): JsonObject {
  const payload: JsonObject = {}
  for (const [key, value] of Object.entries(part)) {
    if (key === "type") continue
    payload[key] = value
  }
  return payload
}

export function toolMessageFromPayload(
  baseMessage: RemoteMessage,
  payload: JsonObject,
  cwd: string
): ToolMessage {
  const toolMessage: RemoteMessage = {
    ...baseMessage,
    role: "tool",
    content: JSON.stringify(payload),
    reasoning: null,
    parts: [],
  }
  return parseToolMessage(toolMessage, cwd)
}

export function mergeAssistantTurnMessages(base: RemoteMessage, next: RemoteMessage): RemoteMessage {
  return {
    ...base,
    content: joinMessageParts(base.content, next.content),
    reasoning: joinOptionalMessageParts(base.reasoning, next.reasoning),
    parts: [...(base.parts || []), ...(next.parts || [])],
    is_complete: next.is_complete,
    agent_mode: next.agent_mode ?? base.agent_mode,
    token_count: next.token_count ?? base.token_count,
    duration_ms: next.duration_ms ?? base.duration_ms,
    t0_ms: next.t0_ms ?? base.t0_ms,
    t1_ms: next.t1_ms ?? base.t1_ms,
    tn_ms: next.tn_ms ?? base.tn_ms,
    output_tokens: next.output_tokens ?? base.output_tokens,
    model: next.model ?? base.model,
    provider: next.provider ?? base.provider,
    local_image_paths: [...base.local_image_paths, ...next.local_image_paths],
    was_interrupted: base.was_interrupted || next.was_interrupted,
  }
}

export function joinOptionalMessageParts(
  first: string | null,
  second: string | null
): string | null {
  const joined = joinMessageParts(first || "", second || "")
  return joined.length > 0 ? joined : null
}

export function joinMessageParts(first: string, second: string): string {
  const left = first.trimEnd()
  const right = second.trimStart()
  if (!left) return right
  if (!right) return left
  return `${left}\n\n${right}`
}

export function parseToolMessage(message: RemoteMessage, cwd: string): ToolMessage {
  const obj = parseJsonObject(message.content)
  const parsed: ParsedToolMessage = {
    id: stringValue(obj?.id) || stringValue(obj?.call_id) || cuid(),
    name: stringValue(obj?.name) || "tool",
    status: stringValue(obj?.status) || "ok",
    args: obj?.args,
    metadata: obj?.metadata,
    outputPreview: stringValue(obj?.output_preview),
    title: stringValue(obj?.title),
    lineCount: numberValue(obj?.line_count),
  }

  if (!obj && message.content.trim()) {
    parsed.outputPreview = message.content
  }

  return { message, parsed, cwd }
}

export function parseJsonObject(content: string): JsonObject | null {
  try {
    const value = JSON.parse(content) as JsonValue
    return asObject(value) ?? null
  } catch {
    return null
  }
}

export function isActivityTool(tool: ToolMessage) {
  return THINKING_TOOL_NAMES.has(tool.parsed.name) && !ACTION_TOOL_NAMES.has(tool.parsed.name)
}

export function buildActivitySteps(tools: ToolMessage[]): ToolActivityStep[] {
  const steps: ToolActivityStep[] = []
  let exploration: ToolMessage[] = []

  const flushExploration = () => {
    if (exploration.length === 0) return
    steps.push(explorationActivityStep(exploration, steps.length))
    exploration = []
  }

  for (const tool of tools) {
    if (EXPLORATION_TOOL_NAMES.has(tool.parsed.name)) {
      exploration.push(tool)
    } else {
      flushExploration()
      steps.push(activityStepFromTool(tool, steps.length))
    }
  }

  flushExploration()
  return steps
}

export function explorationActivityStep(tools: ToolMessage[], index: number): ToolActivityStep {
  const details = tools.map(explorationDetail)
  const state = combinedToolState(tools)
  const count = Math.max(1, details.length)
  const first = details[0]?.label ?? "Explored files"
  const label =
    state === "active"
      ? count === 1
        ? first.replace(/^Read /, "Reading ").replace(/^Listed /, "Listing ").replace(/^Searched /, "Searching ")
        : `Exploring ${formatCount(count, "file")}`
      : state === "error"
        ? count === 1
          ? `${first} failed`
          : "File exploration failed"
        : count === 1
          ? first
          : `Explored ${formatCount(count, "file")}`

  return {
    key: `exploration-${index}-${tools.map((tool) => tool.parsed.id).join("-")}`,
    label,
    icon: "search",
    state,
    details,
    defaultOpen: state !== "complete" || details.length > 1,
  }
}

export function explorationDetail(tool: ToolMessage): ToolStepDetail {
  const args = asObject(tool.parsed.args)
  const title = tool.parsed.title
  const status = toolState(tool)

  if (tool.parsed.name === "read") {
    const path = argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(title, "Read")
    return {
      label: `Read ${displayPath(path || "file", tool.cwd, true)}`,
      detail: firstPreviewLine(tool.parsed.outputPreview),
      status,
    }
  }

  if (tool.parsed.name === "list") {
    const path = argString(args, ["path"]) || stripToolTitle(title, "List") || "."
    return {
      label: `Listed ${displayPath(path, tool.cwd, false)}`,
      detail: firstPreviewLine(tool.parsed.outputPreview),
      status,
    }
  }

  const query =
    argString(args, ["pattern", "query"]) ||
    stripToolTitle(title, tool.parsed.name === "glob" ? "Glob" : "Grep") ||
    "workspace"
  const path = argString(args, ["path"])
  const include = argString(args, ["include"])
  return {
    label: `Searched ${query}`,
    detail: [path ? displayPath(path, tool.cwd, false) : "", include ? `include=${include}` : ""]
      .filter(Boolean)
      .join(" "),
    status,
  }
}

export function activityStepFromTool(tool: ToolMessage, index: number): ToolActivityStep {
  const args = asObject(tool.parsed.args)
  const metadata = asObject(tool.parsed.metadata)
  const state = toolState(tool)
  const key = `${tool.parsed.name}-${tool.parsed.id}-${index}`

  if (tool.parsed.name === "webfetch") {
    const url =
      argString(metadata, ["url"]) ||
      argString(args, ["url"]) ||
      stripToolTitle(tool.parsed.title, "Fetched") ||
      "source"
    return {
      key,
      label: state === "active" ? "Searching web" : state === "error" ? "Web search failed" : "Searched web",
      icon: "globe",
      state,
      details: [{ label: readableUrl(url), detail: firstPreviewLine(tool.parsed.outputPreview), status: state }],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "view_image") {
    const path = argString(metadata, ["path"]) || argString(args, ["path"]) || "image"
    const width = numberValue(metadata?.width)
    const height = numberValue(metadata?.height)
    return {
      key,
      label: state === "active" ? "Viewing image" : state === "error" ? "Image view failed" : "Viewed image",
      icon: "file",
      state,
      details: [
        {
          label: displayPath(path, tool.cwd, true),
          detail: width && height ? `${width} x ${height}` : undefined,
          status: state,
        },
      ],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "skill") {
    const name = argString(metadata, ["name"]) || argString(args, ["name"]) || stripToolTitle(tool.parsed.title, "Loaded skill")
    const resources = arrayValue(metadata?.resources)
    return {
      key,
      label:
        state === "active"
          ? `Loading skill${name ? ` ${name}` : ""}`
          : state === "error"
            ? "Skill load failed"
            : `Loaded skill${name ? ` ${name}` : ""}`,
      icon: "brain",
      state,
      details: resources.length > 0 ? [{ label: formatCount(resources.length, "resource"), status: state }] : [],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "task") {
    const subagent = argString(metadata, ["subagent_type"]) || argString(args, ["subagent_type"]) || "agent"
    const description =
      argString(metadata, ["child_session_title"]) || argString(args, ["description"]) || firstPreviewLine(tool.parsed.outputPreview)
    return {
      key,
      label:
        state === "active"
          ? `Running ${formatToolName(subagent)} agent`
          : state === "error"
            ? `${formatToolName(subagent)} agent failed`
            : `Ran ${formatToolName(subagent)} agent`,
      icon: "brain",
      state,
      details: description ? [{ label: description, status: state }] : [],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "update_plan" || tool.parsed.name === "todowrite") {
    const planDetails = planStepDetails(tool)
    return {
      key,
      label: state === "active" ? "Updating plan" : state === "error" ? "Plan update failed" : "Updated plan",
      icon: "check",
      state,
      details: planDetails,
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "question") {
    const questions = questionDetails(tool)
    return {
      key,
      label:
        state === "active"
          ? formatCount(Math.max(questions.length, 1), "question", "Asking")
          : state === "error"
            ? "Question failed"
            : formatCount(Math.max(questions.length, 1), "question", "Answered"),
      icon: "brain",
      state,
      details: questions,
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  return {
    key,
    label:
      state === "active"
        ? `Running ${formatToolName(tool.parsed.name)}`
        : state === "error"
          ? `${formatToolName(tool.parsed.name)} failed`
          : formatToolName(tool.parsed.title || tool.parsed.name),
    icon: state === "error" ? "warning" : "brain",
    state,
    details: genericToolDetails(tool),
    preview: tool.parsed.outputPreview,
    defaultOpen: state !== "complete",
  }
}

export function actionDescriptor(tool: ToolMessage): ActionDescriptor {
  const state = toolState(tool)
  const args = asObject(tool.parsed.args)
  const metadata = asObject(tool.parsed.metadata)
  const errorPreview = state === "error" ? tool.parsed.outputPreview : undefined

  if (tool.parsed.name === "edit") {
    const filePath =
      argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(tool.parsed.title, "Edit") || "file"
    const oldText = argString(args, ["old_string", "oldString"]) || ""
    const newText = argString(args, ["new_string", "newString"]) || ""
    return {
      label: state === "active" ? "Editing" : state === "error" ? "Edit failed" : "Edited",
      description: displayPath(filePath, tool.cwd, false),
      state,
      icon: state === "error" ? "warning" : "pencil",
      stats: diffStats(oldText, newText),
      details: [
        {
          label: displayPath(filePath, tool.cwd, false),
          detail: lineNumberDetail(metadata, tool.parsed.outputPreview),
          status: state,
        },
      ],
      diffLines: withDiffLanguage(compactDiffLines(diffLineOps(oldText, newText)), filePath),
      preview: errorPreview,
    }
  }

  if (tool.parsed.name === "write") {
    const filePath =
      argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(tool.parsed.title, "Write") || "file"
    const newText = argString(args, ["content"]) || ""
    const created = tool.parsed.outputPreview?.startsWith("Created file")
    return {
      label: state === "active" ? "Writing" : state === "error" ? "Write failed" : created ? "Added" : "Edited",
      description: displayPath(filePath, tool.cwd, false),
      state,
      icon: state === "error" ? "warning" : "pencil",
      stats: diffStats("", newText),
      details: [
        {
          label: displayPath(filePath, tool.cwd, false),
          detail: firstPreviewLine(tool.parsed.outputPreview),
          status: state,
        },
      ],
      diffLines: withDiffLanguage(compactDiffLines(diffLineOps("", newText)), filePath),
      preview: errorPreview,
    }
  }

  if (tool.parsed.name === "apply_patch") {
    const patch = argString(args, ["patch"]) || ""
    const patchPreview = patchPreviewFromText(patch, tool.cwd)
    const paths = patchPreview.paths.length > 0 ? patchPreview.paths : patchPaths(patch, tool.cwd)
    const fileCount = numberValue(metadata?.file_count) ?? paths.length
    const description = paths.length > 0 ? paths.slice(0, 3).join(", ") : fileCount > 0 ? formatCount(fileCount, "file") : tool.parsed.title || "Workspace patch"
    return {
      label: state === "active" ? "Applying patch" : state === "error" ? "Patch failed" : "Applied patch",
      description: paths.length > 3 ? `${description} +${paths.length - 3} more` : description,
      state,
      icon: state === "error" ? "warning" : "pencil",
      stats: { added: patchPreview.added, removed: patchPreview.removed },
      details:
        paths.length > 0
          ? paths.slice(0, 8).map((path) => ({ label: path, status: state }))
          : [{ label: tool.parsed.title || "Patch", detail: firstPreviewLine(tool.parsed.outputPreview), status: state }],
      diffLines: patchPreview.sections.length === 1 ? patchPreview.sections[0].lines : [],
      diffSections: patchPreview.sections.length > 1 ? patchPreview.sections : undefined,
      preview: state === "error" ? tool.parsed.outputPreview : patchPreview.sections.length > 0 ? undefined : firstPreviewLine(tool.parsed.outputPreview),
    }
  }

  if (tool.parsed.name === "bash") {
    const command = argString(metadata, ["command"]) || argString(args, ["command"]) || stripToolTitle(tool.parsed.title, "Bash") || "command"
    const exitCode = numberValue(metadata?.exit_code)
    return {
      label: state === "active" ? "Running command" : state === "error" ? "Command failed" : "Ran command",
      description: command,
      state,
      icon: state === "error" ? "warning" : "terminal",
      details: [
        {
          label: exitCode === undefined ? "Shell" : `Exit ${exitCode}`,
          detail: command,
          status: state,
        },
      ],
      diffLines: [],
      preview: tool.parsed.outputPreview,
    }
  }

  return {
    label:
      state === "active"
        ? `Running ${formatToolName(tool.parsed.name)}`
        : state === "error"
          ? `${formatToolName(tool.parsed.name)} failed`
          : formatToolName(tool.parsed.title || tool.parsed.name),
    description: firstPreviewLine(tool.parsed.outputPreview) || "Tool call",
    state,
    icon: state === "error" ? "warning" : "terminal",
    details: genericToolDetails(tool),
    diffLines: [],
    preview: tool.parsed.outputPreview,
  }
}

export function planStepDetails(tool: ToolMessage): ToolStepDetail[] {
  const metadata = asObject(tool.parsed.metadata)
  const args = asObject(tool.parsed.args)
  const value = metadata?.plan ?? metadata?.todo_items ?? args?.plan ?? args?.todos
  const steps = arrayValue(value)
    .map((item): ToolStepDetail | null => {
      if (typeof item === "string") return { label: item.trim(), status: "complete" as ToolVisualState }
      const obj = asObject(item)
      const label = stringValue(obj?.step) || stringValue(obj?.content) || stringValue(obj?.title) || stringValue(obj?.description)
      if (!label) return null
      const rawStatus = stringValue(obj?.status)
      const status: ToolVisualState | undefined =
        rawStatus === "completed" || rawStatus === "complete" || rawStatus === "done"
          ? "complete"
          : rawStatus === "in_progress" || rawStatus === "active"
            ? "active"
            : undefined
      return {
        label,
        status,
      }
    })
    .filter((item): item is ToolStepDetail => item !== null && item.label.length > 0)

  if (steps.length > 0) return steps.slice(0, 8)

  return firstPreviewLine(tool.parsed.outputPreview)
    ? [{ label: firstPreviewLine(tool.parsed.outputPreview) ?? "Plan updated", status: toolState(tool) }]
    : []
}

export function questionDetails(tool: ToolMessage): ToolStepDetail[] {
  const metadata = asObject(tool.parsed.metadata)
  const args = asObject(tool.parsed.args)
  const questions = arrayValue(metadata?.questions ?? args?.questions)
  return questions
    .map((question, index) => {
      const obj = asObject(question)
      const label =
        stringValue(obj?.question) ||
        stringValue(obj?.prompt) ||
        stringValue(obj?.header) ||
        (typeof question === "string" ? question : `Question ${index + 1}`)
      return { label, status: toolState(tool) }
    })
    .slice(0, 6)
}

export function genericToolDetails(tool: ToolMessage): ToolStepDetail[] {
  const details: ToolStepDetail[] = []
  if (tool.parsed.title) details.push({ label: tool.parsed.title, status: toolState(tool) })
  const argsPreview = tool.parsed.args ? jsonSummary(tool.parsed.args) : ""
  if (argsPreview) details.push({ label: "Input", detail: argsPreview, status: toolState(tool) })
  return details
}

export function combinedToolState(tools: ToolMessage[]): ToolVisualState {
  if (tools.some((tool) => toolState(tool) === "error")) return "error"
  if (tools.some((tool) => toolState(tool) === "active")) return "active"
  return "complete"
}

export function toolState(tool: ToolMessage): ToolVisualState {
  const status = tool.parsed.status.toLowerCase()
  if (status === "error" || status === "failed") return "error"
  if (status === "running" || status === "pending") return "active"
  return "complete"
}

export function asObject(value: JsonValue | undefined): JsonObject | undefined {
  if (value && typeof value === "object" && !Array.isArray(value)) return value
  return undefined
}

export function arrayValue(value: JsonValue | undefined): JsonValue[] {
  return Array.isArray(value) ? value : []
}

export function stringValue(value: JsonValue | undefined): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined
}

export function numberValue(value: JsonValue | undefined): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined
}

export function argString(obj: JsonObject | undefined, keys: string[]) {
  for (const key of keys) {
    const value = stringValue(obj?.[key])
    if (value) return value
  }
  return undefined
}

export function stripToolTitle(title: string | undefined, label: string) {
  const prefix = `${label}:`
  return title?.startsWith(prefix) ? title.slice(prefix.length).trim() || undefined : undefined
}

export function displayPath(raw: string, cwd: string, basenameOnly: boolean) {
  const trimmed = raw.trim() || "."
  if (basenameOnly) return basename(trimmed)
  if (cwd && trimmed === cwd) return "."
  if (cwd && trimmed.startsWith(`${cwd}/`)) return trimmed.slice(cwd.length + 1)
  return trimmed.replace(/^file:\/\//, "")
}

export function readableUrl(raw: string) {
  try {
    const url = new URL(raw)
    return url.hostname.replace(/^www\./, "") + url.pathname.replace(/\/$/, "")
  } catch {
    return raw
  }
}

export function firstPreviewLine(preview: string | undefined) {
  return preview
    ?.split("\n")
    .map((line) => line.trim())
    .find(Boolean)
}

export function formatCount(count: number, noun: string, verb?: string) {
  const label = `${count} ${noun}${count === 1 ? "" : "s"}`
  return verb ? `${verb} ${label}` : label
}

export function formatToolName(value: string) {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase())
}

export function trimPreview(preview: string, maxChars = 1200) {
  const trimmed = preview.trim()
  return trimmed.length > maxChars ? `${trimmed.slice(0, maxChars).trimEnd()}\n...` : trimmed
}

export function jsonSummary(value: JsonValue) {
  try {
    return trimPreview(JSON.stringify(value, null, 2), 420)
  } catch {
    return ""
  }
}

export function lineNumberDetail(metadata: JsonObject | undefined, preview: string | undefined) {
  const line = numberValue(metadata?.line_number) ?? numberValue(metadata?.line) ?? numberValue(metadata?.start_line)
  if (line) return `line ${line}`
  return firstPreviewLine(preview)
}

export function splitLines(text: string) {
  if (!text) return []
  const normalized = text.endsWith("\n") ? text.slice(0, -1) : text
  return normalized ? normalized.split("\n") : []
}

export function diffStats(oldText: string, newText: string) {
  const oldLines = splitLines(oldText)
  const newLines = splitLines(newText)
  const lcs = lcsLength(oldLines, newLines)
  return {
    added: Math.max(0, newLines.length - lcs),
    removed: Math.max(0, oldLines.length - lcs),
  }
}

export function diffLineOps(oldText: string, newText: string): DiffLine[] {
  const oldLines = splitLines(oldText)
  const newLines = splitLines(newText)

  if (oldLines.length === 0) return newLines.map((text) => ({ kind: "add", text }))
  if (newLines.length === 0) return oldLines.map((text) => ({ kind: "remove", text }))
  if (oldLines.length * newLines.length > 20000) {
    return [
      ...oldLines.slice(0, 4).map((text) => ({ kind: "remove" as const, text })),
      ...newLines.slice(0, 4).map((text) => ({ kind: "add" as const, text })),
    ]
  }

  const dp = lcsMatrix(oldLines, newLines)
  const ops: DiffLine[] = []
  let i = 0
  let j = 0

  while (i < oldLines.length && j < newLines.length) {
    if (oldLines[i] === newLines[j]) {
      ops.push({ kind: "context", text: oldLines[i] })
      i += 1
      j += 1
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push({ kind: "remove", text: oldLines[i] })
      i += 1
    } else {
      ops.push({ kind: "add", text: newLines[j] })
      j += 1
    }
  }

  while (i < oldLines.length) ops.push({ kind: "remove", text: oldLines[i++] })
  while (j < newLines.length) ops.push({ kind: "add", text: newLines[j++] })
  return ops
}

export function compactDiffLines(lines: DiffLine[], maxLines = 12) {
  const changed = lines
    .map((line, index) => (line.kind === "context" ? -1 : index))
    .filter((index) => index >= 0)

  if (changed.length === 0) return lines.slice(0, Math.min(lines.length, maxLines))

  const start = Math.max(0, changed[0] - 2)
  const end = Math.min(lines.length, changed[changed.length - 1] + 3)
  return lines.slice(start, end).slice(0, maxLines)
}

export function withDiffLanguage(lines: DiffLine[], path: string) {
  const language = languageForPath(path)
  return lines.map((line) => ({ ...line, language }))
}

type PatchMode =
  | { kind: "none" }
  | { kind: "add"; newLine: number }
  | { kind: "hunk"; oldLine?: number; newLine?: number }

type PatchPreview = {
  paths: string[]
  sections: DiffSection[]
  added: number
  removed: number
}

const PATCH_DIFF_MAX_LINES = 80

export function patchPreviewFromText(patch: string, cwd: string): PatchPreview {
  const paths = patchPaths(patch, cwd)
  const sections: DiffSection[] = []
  const lines = patchLinesWithoutFences(patch)
  let mode: PatchMode = { kind: "none" }
  let current: DiffSection | undefined
  let added = 0
  let removed = 0
  let totalLines = 0

  const sectionForPath = (rawPath: string) => {
    const path = displayPath(normalizeDiffPath(rawPath), cwd, false)
    let section = sections.find((item) => item.path === path)
    if (!section) {
      section = { path, language: languageForPath(path), lines: [] }
      sections.push(section)
    }
    current = section
    return section
  }

  const pushLine = (kind: DiffLine["kind"], text: string, lineNumber?: number) => {
    const section = current || sectionForPath(paths[0] || "Patch")
    if (kind === "add") added += 1
    if (kind === "remove") removed += 1
    if (totalLines >= PATCH_DIFF_MAX_LINES) return
    section.lines.push({ kind, text, lineNumber, language: section.language })
    totalLines += 1
  }

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]
    const trimmed = line.trim()
    const next = lines[index + 1]

    if (!trimmed || trimmed === "\\ No newline at end of file") continue

    const addFile = trimmed.match(/^\*\*\* Add File: (.+)$/)?.[1]
    if (addFile) {
      sectionForPath(addFile)
      mode = { kind: "add", newLine: 1 }
      continue
    }

    const codexPath = trimmed.match(/^\*\*\* (?:Update|Delete) File: (.+)$/)?.[1] || trimmed.match(/^\*\*\* Move to: (.+)$/)?.[1]
    if (codexPath) {
      sectionForPath(codexPath)
      mode = { kind: "none" }
      continue
    }

    if (trimmed === "*** Begin Patch" || trimmed === "*** End Patch") continue

    if (line.startsWith("--- ") && next?.startsWith("+++ ")) {
      const plusPath = normalizeDiffPath(next.slice(4))
      const minusPath = normalizeDiffPath(line.slice(4))
      sectionForPath(plusPath === "/dev/null" ? minusPath : plusPath)
      mode = { kind: "none" }
      continue
    }
    if (line.startsWith("+++ ") || line.startsWith("diff --git ") || line.startsWith("index ") || line.startsWith("new file mode ") || line.startsWith("deleted file mode ")) {
      mode = { kind: "none" }
      continue
    }

    if (line.startsWith("@@")) {
      const { oldLine, newLine } = parsePatchHunkStart(line)
      mode = { kind: "hunk", oldLine, newLine }
      continue
    }

    if (mode.kind === "add") {
      if (line.startsWith("+")) pushLine("add", line.slice(1), mode.newLine++)
      continue
    }

    if (mode.kind === "hunk") {
      const prefix = line[0]
      const text = line.slice(1)
      if (prefix === " ") {
        pushLine("context", text, mode.newLine)
        if (mode.oldLine !== undefined) mode.oldLine += 1
        if (mode.newLine !== undefined) mode.newLine += 1
      } else if (prefix === "-") {
        pushLine("remove", text, mode.oldLine)
        if (mode.oldLine !== undefined) mode.oldLine += 1
      } else if (prefix === "+") {
        pushLine("add", text, mode.newLine)
        if (mode.newLine !== undefined) mode.newLine += 1
      }
    }
  }

  return {
    paths: paths.length > 0 ? paths : sections.map((section) => section.path),
    sections: sections.filter((section) => section.lines.length > 0),
    added,
    removed,
  }
}

function patchLinesWithoutFences(patch: string) {
  const lines = patch.trim().split("\n")
  if (lines[0]?.trimStart().startsWith("```")) lines.shift()
  if (lines[lines.length - 1]?.trimStart().startsWith("```")) lines.pop()
  return lines
}

function normalizeDiffPath(raw: string) {
  const path = raw.trim().split(/\s+/)[0]?.replace(/^"|"$/g, "") || ""
  return path.replace(/^[ab]\//, "")
}

function parsePatchHunkStart(line: string) {
  const oldLine = line.match(/ -(\d+)/)?.[1]
  const newLine = line.match(/ \+(\d+)/)?.[1]
  return {
    oldLine: oldLine ? Math.max(1, Number(oldLine)) : undefined,
    newLine: newLine ? Math.max(1, Number(newLine)) : undefined,
  }
}

export function languageForPath(path: string) {
  const ext = path.split(".").pop()?.toLowerCase()
  if (!ext) return undefined
  if (["ts", "tsx", "js", "jsx", "mjs", "cjs"].includes(ext)) return "typescript"
  if (ext === "rs") return "rust"
  if (["json", "jsonc"].includes(ext)) return "json"
  if (["md", "mdx"].includes(ext)) return "markdown"
  if (["css", "scss", "sass"].includes(ext)) return "css"
  if (["html", "xml"].includes(ext)) return "html"
  return ext
}

export function lcsLength(left: string[], right: string[]) {
  if (left.length * right.length > 20000) return 0
  const dp = lcsMatrix(left, right)
  return dp[0][0]
}

export function lcsMatrix(left: string[], right: string[]) {
  const dp = Array.from({ length: left.length + 1 }, () => Array(right.length + 1).fill(0))
  for (let i = left.length - 1; i >= 0; i -= 1) {
    for (let j = right.length - 1; j >= 0; j -= 1) {
      dp[i][j] = left[i] === right[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
    }
  }
  return dp
}

export function patchPaths(patch: string, cwd: string) {
  const paths = new Set<string>()
  for (const line of patch.split("\n")) {
    const codexPath =
      line.match(/^\*\*\* (?:Update|Add|Delete) File: (.+)$/)?.[1] ||
      line.match(/^\+\+\+ b\/(.+)$/)?.[1] ||
      line.match(/^--- a\/(.+)$/)?.[1]
    if (codexPath && codexPath !== "/dev/null") paths.add(displayPath(codexPath, cwd, false))
  }
  return [...paths]
}

export function messageModelLabel(message: RemoteMessage, status: RemoteStatus | null) {
  const model = message.model || status?.model || "model"
  const provider = message.provider || status?.provider || ""
  if (!provider) return model
  if (model.startsWith(`${provider}/`)) return model
  return `${provider}/${model}`
}

export function assistantMetrics(message: RemoteMessage) {
  if (!message.is_complete) return []
  const metrics: string[] = []

  if (message.t0_ms != null && message.t1_ms != null && message.tn_ms != null) {
    const totalMs = Math.max(0, message.tn_ms - message.t0_ms)
    const ttftMs = Math.max(0, message.t1_ms - message.t0_ms)
    const decodeMs = Math.max(0, message.tn_ms - message.t1_ms)
    const tokens = message.output_tokens ?? message.token_count ?? 0
    metrics.push(formatSeconds(totalMs))
    metrics.push(`ttft ${formatSeconds(ttftMs)}`)
    if (decodeMs > 0 && tokens > 0) metrics.push(`${Math.round(tokens / (decodeMs / 1000))}t/s`)
  } else if (message.token_count != null && message.duration_ms != null) {
    metrics.push(formatSeconds(message.duration_ms))
    if (message.duration_ms > 0) {
      metrics.push(`${Math.round(message.token_count / (message.duration_ms / 1000))}t/s`)
    }
  }

  if (message.was_interrupted) metrics.push("interrupted")
  return metrics
}

export function sessionTranscript(title: string, messages: RemoteMessage[]) {
  const parts = [`# ${title || "Untitled"}`]

  for (const message of messages) {
    if (message.role === "system") continue
    if (message.role === "user") {
      parts.push(`## User\n\n${message.content}`)
      continue
    }
    if (message.role === "assistant") {
      const agent = message.agent_mode || "Build"
      const model = message.model || "unknown"
      parts.push(`## Assistant (${agent} · ${model})\n\n${message.content}`)
      continue
    }
    if (message.role === "tool") {
      parts.push(`**Tool Result**\n\n${formatToolTranscript(message.content)}`)
    }
  }

  return `${parts.join("\n\n---\n\n")}\n`
}

export function formatToolTranscript(content: string) {
  try {
    const value = JSON.parse(content) as JsonValue
    return `\`\`\`json\n${JSON.stringify(value, null, 2)}\n\`\`\``
  } catch {
    return `\`\`\`\n${content}\n\`\`\``
  }
}
