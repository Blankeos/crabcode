import type { JSX } from "solid-js"
import type { AttachmentData } from "../../components/ai-elements/attachments"
import type { RemoteMessage } from "../../remote-api"
import { IMAGE_FILE_TYPES, MAX_PROMPT_HISTORY, MENTION_ACCENTS } from "./page-constants"
import type { CompletionTrigger, ComposerAttachment, ImagePlaceholderRange, ImagePreviewTarget, PromptTextPart } from "./page-types"
import { basename, cuid } from "./shared-utils"

const PROMPT_HISTORY_KEY = "crabcode.remote.promptHistory"

export function loadPromptHistory() {
  try {
    const parsed = JSON.parse(localStorage.getItem(PROMPT_HISTORY_KEY) || "[]") as unknown
    if (!Array.isArray(parsed)) return []
    return mergePromptHistoryEntries(parsed.filter((item): item is string => typeof item === "string"))
  } catch {
    return []
  }
}

export function savePromptHistory(entries: string[]) {
  try {
    localStorage.setItem(PROMPT_HISTORY_KEY, JSON.stringify(entries.slice(0, MAX_PROMPT_HISTORY)))
  } catch {
    // Losing local browser history should not block chat input.
  }
}

export function messagePromptHistoryEntries(messages: RemoteMessage[]) {
  return messages
    .filter((message) => message.role === "user")
    .map((message) => message.content)
    .reverse()
}

export function mergePromptHistoryEntries(...groups: string[][]) {
  const seen = new Set<string>()
  const entries: string[] = []

  for (const text of groups.flat()) {
    const entry = normalizePromptHistoryEntry(text)
    if (!entry || seen.has(entry) || parseSlashCommand(entry)) continue
    seen.add(entry)
    entries.push(entry)
    if (entries.length >= MAX_PROMPT_HISTORY) break
  }

  return entries
}

export function normalizePromptHistoryEntry(text: string) {
  return text.trim()
}

export function isCursorOnFirstPromptLine(textarea: HTMLTextAreaElement, text: string, cursor: number) {
  const visualLine = promptCursorVisualLine(textarea, text, cursor)
  if (!visualLine) return isCursorOnFirstLogicalLine(text, cursor)
  return sameVisualLine(visualLine.cursorTop, visualLine.firstTop, visualLine.lineHeight)
}

export function isCursorOnLastPromptLine(textarea: HTMLTextAreaElement, text: string, cursor: number) {
  const visualLine = promptCursorVisualLine(textarea, text, cursor)
  if (!visualLine) return isCursorOnLastLogicalLine(text, cursor)
  return sameVisualLine(visualLine.cursorTop, visualLine.lastTop, visualLine.lineHeight)
}

export function isCursorOnFirstLogicalLine(text: string, cursor: number) {
  return !text.slice(0, Math.max(0, cursor)).includes("\n")
}

export function isCursorOnLastLogicalLine(text: string, cursor: number) {
  return !text.slice(Math.max(0, cursor)).includes("\n")
}

export function promptCursorVisualLine(textarea: HTMLTextAreaElement, text: string, cursor: number) {
  if (typeof document === "undefined") return null

  // Mirror textarea wrapping so history navigation does not steal ArrowUp/Down from visual rows.
  const style = window.getComputedStyle(textarea)
  const mirror = document.createElement("div")
  const firstMarker = document.createElement("span")
  const cursorMarker = document.createElement("span")
  const lastMarker = document.createElement("span")
  const clampedCursor = Math.max(0, Math.min(cursor, text.length))

  for (const property of [
    "box-sizing",
    "border-bottom-width",
    "border-left-width",
    "border-right-width",
    "border-top-width",
    "font-family",
    "font-feature-settings",
    "font-kerning",
    "font-size",
    "font-stretch",
    "font-style",
    "font-variant",
    "font-variant-ligatures",
    "font-weight",
    "letter-spacing",
    "line-height",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "tab-size",
    "text-align",
    "text-indent",
    "text-rendering",
    "text-transform",
    "width",
    "word-break",
  ]) {
    mirror.style.setProperty(property, style.getPropertyValue(property))
  }

  mirror.style.position = "absolute"
  mirror.style.visibility = "hidden"
  mirror.style.pointerEvents = "none"
  mirror.style.top = "0"
  mirror.style.left = "-9999px"
  mirror.style.overflow = "hidden"
  mirror.style.whiteSpace = "pre-wrap"
  mirror.style.overflowWrap = "break-word"

  firstMarker.textContent = "\u200b"
  cursorMarker.textContent = "\u200b"
  lastMarker.textContent = "\u200b"

  mirror.append(
    firstMarker,
    document.createTextNode(text.slice(0, clampedCursor)),
    cursorMarker,
    document.createTextNode(text.slice(clampedCursor)),
    lastMarker
  )

  document.body.append(mirror)
  const lineHeight = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2 || 16
  const result = {
    cursorTop: cursorMarker.offsetTop,
    firstTop: firstMarker.offsetTop,
    lastTop: lastMarker.offsetTop,
    lineHeight,
  }
  mirror.remove()
  return result
}

export function sameVisualLine(leftTop: number, rightTop: number, lineHeight: number) {
  return Math.abs(leftTop - rightTop) <= Math.max(1, lineHeight / 4)
}

export function isSupportedImageFile(file: File) {
  return IMAGE_FILE_TYPES.includes(file.type)
}

export function readComposerAttachment(file: File): Promise<ComposerAttachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      if (typeof reader.result !== "string" || !reader.result.startsWith("data:image/")) {
        reject(new Error("Could not read image."))
        return
      }

      resolve({
        id: cuid(),
        name: file.name || "pasted-image.png",
        mediaType: file.type || mediaTypeFromDataUrl(reader.result),
        size: file.size,
        dataUrl: reader.result,
      })
    }
    reader.onerror = () => reject(new Error("Could not read image."))
    reader.readAsDataURL(file)
  })
}

export function filesFromClipboard(clipboardData: DataTransfer | null) {
  if (!clipboardData) return []

  const files = Array.from(clipboardData.files ?? []).filter((file) => file.type.startsWith("image/"))
  if (files.length > 0) return files

  return Array.from(clipboardData.items ?? [])
    .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file))
}

export function promptTextWithAttachmentPlaceholders(rawText: string, attachmentCount: number) {
  if (attachmentCount <= 0) return rawText

  let text = rawText
  for (let index = 1; index <= attachmentCount; index += 1) {
    const placeholder = `[Image #${index}]`
    if (!text.includes(placeholder)) {
      if (text.length > 0 && !/\s$/.test(text)) text += " "
      text += placeholder
    }
  }
  return text
}

export function promptTextParts(text: string, attachmentCount: number): PromptTextPart[] {
  const parts: PromptTextPart[] = []
  let cursor = 0
  const ranges = [
    ...imagePlaceholderRanges(text)
      .filter((range) => range.number >= 1 && range.number <= attachmentCount)
      .map((range) => ({ kind: "image" as const, start: range.start, end: range.end })),
    ...agentMentionRanges(text).map((range) => ({ kind: "mention" as const, start: range.start, end: range.end })),
  ].sort((left, right) => left.start - right.start || left.end - right.end)

  for (const range of ranges) {
    if (range.start < cursor) continue
    if (range.start > cursor) {
      parts.push({ kind: "text", text: text.slice(cursor, range.start) })
    }
    parts.push({ kind: range.kind, text: text.slice(range.start, range.end) })
    cursor = range.end
  }

  if (cursor < text.length) {
    parts.push({ kind: "text", text: text.slice(cursor) })
  }

  return parts
}

export function agentMentionRanges(text: string): Array<{ start: number; end: number }> {
  return Array.from(text.matchAll(/(^|[\s([{])(@[A-Za-z0-9][A-Za-z0-9_-]*)/g)).map((match) => {
    const prefixLength = match[1]?.length ?? 0
    const start = (match.index ?? 0) + prefixLength
    return {
      start,
      end: start + match[2].length,
    }
  })
}

export function promptTextPartClass(part: PromptTextPart) {
  if (part.kind === "image") {
    return "rounded-[4px] bg-[rgba(118,185,145,0.14)] text-[#9ed8b7] shadow-[0_0_0_1px_rgba(118,185,145,0.18)]"
  }
  if (part.kind === "mention") {
    return "rounded-[4px]"
  }
  return undefined
}

export function promptTextPartStyle(part: PromptTextPart): JSX.CSSProperties | undefined {
  if (part.kind !== "mention") return undefined
  const accent = mentionAccent(part.text)
  return {
    color: accent.text,
    "background-color": accent.background,
    "box-shadow": `0 0 0 1px ${accent.ring}`,
  } as JSX.CSSProperties
}

export function mentionAccent(text: string) {
  const key = text.replace(/^@/, "").toLowerCase()
  let hash = 0
  for (let index = 0; index < key.length; index += 1) {
    hash = (hash * 31 + key.charCodeAt(index)) >>> 0
  }
  return MENTION_ACCENTS[hash % MENTION_ACCENTS.length]
}

export function imagePlaceholderRanges(text: string): ImagePlaceholderRange[] {
  return Array.from(text.matchAll(/\[Image #(\d+)\]/g)).map((match) => ({
    number: Number(match[1]),
    start: match.index ?? 0,
    end: (match.index ?? 0) + match[0].length,
  }))
}

export function rangesIntersect(leftStart: number, leftEnd: number, rightStart: number, rightEnd: number) {
  return leftStart < rightEnd && rightStart < leftEnd
}

export function removeRangesFromText(text: string, ranges: Array<{ start: number; end: number }>) {
  if (ranges.length === 0) return text

  const sorted = [...ranges]
    .filter((range) => range.end > range.start)
    .sort((left, right) => left.start - right.start || left.end - right.end)
  const merged: Array<{ start: number; end: number }> = []

  for (const range of sorted) {
    const last = merged[merged.length - 1]
    if (last && range.start <= last.end) {
      last.end = Math.max(last.end, range.end)
    } else {
      merged.push({ ...range })
    }
  }

  let output = ""
  let cursor = 0
  for (const range of merged) {
    output += text.slice(cursor, range.start)
    cursor = range.end
  }
  output += text.slice(cursor)
  return output
}

export function renumberImagePlaceholdersAfterRemoval(
  text: string,
  removedNumbers: number[],
  attachmentCount: number
) {
  const removed = new Set(removedNumbers)
  return text
    .replace(/\[Image #(\d+)\]/g, (placeholder, rawNumber) => {
      const number = Number(rawNumber)
      if (!Number.isFinite(number)) return placeholder
      if (number < 1 || number > attachmentCount) return placeholder
      if (removed.has(number)) return ""
      const offset = removedNumbers.filter((removedNumber) => removedNumber < number).length
      if (offset > 0) return `[Image #${number - offset}]`
      return placeholder
    })
    .replace(/[ \t]{2,}/g, " ")
    .replace(/[ \t]+\n/g, "\n")
    .trimStart()
}

export function imagePreviewFromAttachment(attachment: AttachmentData): ImagePreviewTarget | null {
  const mediaType = attachment.mediaType?.toLowerCase() ?? ""
  if (mediaType && !mediaType.startsWith("image/")) return null
  return {
    url: attachment.url,
    label: attachment.filename?.trim() || "Image attachment",
  }
}

export function handleImagePreviewKeyDown(event: KeyboardEvent, onOpen: () => void) {
  if (event.key !== "Enter" && event.key !== " ") return
  event.preventDefault()
  onOpen()
}

export function messageImageAttachmentData(message: RemoteMessage, token: string): AttachmentData[] {
  return (message.local_image_paths ?? []).map((path, index) => ({
    id: `${index}-${path}`,
    url: localImageUrl(path, token),
    filename: `[Image #${index + 1}] ${basename(path) || "image"}`,
    mediaType: imageMediaTypeFromPath(path),
  }))
}

export function localImageUrl(path: string, token: string) {
  const url = new URL("/api/local-image", window.location.origin)
  url.searchParams.set("path", path)
  if (token) url.searchParams.set("token", token)
  return url.toString()
}

export function imageMediaTypeFromPath(path: string) {
  const extension = path.split("?")[0]?.split(".").pop()?.toLowerCase()
  if (extension === "jpg" || extension === "jpeg") return "image/jpeg"
  if (extension === "gif") return "image/gif"
  if (extension === "webp") return "image/webp"
  return "image/png"
}

export function mediaTypeFromDataUrl(dataUrl: string) {
  return dataUrl.slice(5, dataUrl.indexOf(";")) || "image/png"
}

export function detectCompletionTrigger(text: string, cursor: number): CompletionTrigger | null {
  const safeCursor = Math.max(0, Math.min(cursor, text.length))
  const beforeCursor = text.slice(0, safeCursor)

  if (beforeCursor.startsWith("/") && !beforeCursor.includes("\n")) {
    const query = beforeCursor.slice(1)
    if (!query.includes(" ")) return { kind: "slash", query, range: [0, safeCursor] }
  }

  const atIndex = beforeCursor.lastIndexOf("@")
  if (atIndex < 0) return null
  if (atIndex > 0 && !/\s/.test(beforeCursor[atIndex - 1])) return null

  const query = beforeCursor.slice(atIndex + 1)
  if (/\s/.test(query)) return null
  const afterCursor = text.slice(safeCursor)
  const afterToken = afterCursor.search(/\s/)
  const end = afterToken < 0 ? text.length : safeCursor + afterToken
  return { kind: "mention", query, range: [atIndex, end] }
}

export function quoteCompletionPath(path: string) {
  return /\s/.test(path) ? `"${path.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"` : path
}

export function parseSlashCommand(text: string) {
  const trimmed = text.trim()
  if (!trimmed.startsWith("/")) return null
  const body = trimmed.slice(1).trimStart()
  const match = body.match(/^([^\s]+)(?:\s+([\s\S]*))?$/)
  if (!match) return null
  return { name: match[1], args: match[2] ?? "" }
}
