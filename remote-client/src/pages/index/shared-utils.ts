import { type Accessor, createEffect, createSignal, onCleanup, onMount, type Setter } from "solid-js"
import { toast } from "solid-sonner"
import type { RemoteModel } from "../../remote-api"

export function relativeTime(seconds: number) {
  if (!seconds) return ""
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - seconds)
  if (diff < 60) return "now"
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`
  return new Date(seconds * 1000).toLocaleDateString()
}

export function basename(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? ""
}

export function sameToken(left: string, right: string) {
  return left.trim().toLowerCase() === right.trim().toLowerCase()
}

export function showErrorToast(error: unknown, fallback: string) {
  toast.error(errorToastMessage(error, fallback))
}

export function errorToastMessage(error: unknown, fallback: string) {
  if (error instanceof Error && error.message.trim()) return error.message
  return fallback
}

export function fallbackCopyText(text: string) {
  const textarea = document.createElement("textarea")
  textarea.value = text
  textarea.setAttribute("readonly", "")
  textarea.style.position = "fixed"
  textarea.style.opacity = "0"
  textarea.style.pointerEvents = "none"
  document.body.appendChild(textarea)
  textarea.select()
  document.execCommand("copy")
  document.body.removeChild(textarea)
}

export function handleChoiceMenuKeyDown(
  event: KeyboardEvent,
  open: boolean,
  setOpen: Setter<boolean>,
  options: string[],
  activeIndex: number,
  setActiveIndex: Setter<number>,
  onSelect: (value: string) => void | Promise<void>,
  onEscape?: () => void
) {
  if (options.length === 0) return

  if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault()
    setOpen(true)
    setActiveIndex(event.key === "ArrowUp" ? options.length - 1 : Math.max(activeIndex, 0))
    return
  }

  if (!open) return

  if (event.key === "ArrowDown") {
    event.preventDefault()
    setActiveIndex((index) => (index + 1) % options.length)
    return
  }

  if (event.key === "ArrowUp") {
    event.preventDefault()
    setActiveIndex((index) => (index - 1 + options.length) % options.length)
    return
  }

  if (event.key === "Enter") {
    event.preventDefault()
    const selected = options[activeIndex]
    if (selected) void onSelect(selected)
    return
  }

  if (event.key === "Escape") {
    event.preventDefault()
    onEscape?.()
    setOpen(false)
  }
}

export function useStickToBottom(
  scrollEl: Accessor<HTMLElement | undefined>,
  contentEl: Accessor<HTMLElement | undefined>
) {
  const [isAtTop, setIsAtTop] = createSignal(true)
  const [isAtBottom, setIsAtBottom] = createSignal(true)
  const bottomThreshold = 32
  const topThreshold = 8
  let shouldStickToBottom = true

  const measure = () => {
    const el = scrollEl()
    if (!el) return
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight
    const nextIsAtBottom = distance <= bottomThreshold
    setIsAtBottom(nextIsAtBottom)
    shouldStickToBottom = nextIsAtBottom
    setIsAtTop(el.scrollTop <= topThreshold)
  }

  const scrollToBottom = (smooth = false) => {
    const el = scrollEl()
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" })
    measure()
  }

  const scrollTo = (top: number, smooth = false) => {
    const el = scrollEl()
    if (!el) return
    el.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" })
    measure()
  }

  const getScrollTop = () => scrollEl()?.scrollTop ?? 0

  onMount(() => {
    queueMicrotask(() => scrollToBottom(false))
  })

  createEffect(() => {
    const el = scrollEl()
    const content = contentEl()
    if (!el) return
    measure()
    el.addEventListener("scroll", measure, { passive: true })

    const resizeObserver = new ResizeObserver(() => {
      if (shouldStickToBottom) scrollToBottom(false)
      else measure()
    })
    resizeObserver.observe(content ?? el)

    onCleanup(() => {
      el.removeEventListener("scroll", measure)
      resizeObserver.disconnect()
    })
  })

  return { isAtTop, isAtBottom, scrollToBottom, scrollTo, getScrollTop, measure }
}

export function cuid() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID()
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`
}

export function providerLabel(model: RemoteModel) {
  const raw = model.description || model.provider_id
  return raw.split("|")[0].trim() || model.provider_id
}

export function displayAgentMode(agent: string) {
  const normalized = agent.trim() || "Build"
  return normalized
    .split(/([\s_-]+)/)
    .map((part) => (/^[\s_-]+$/.test(part) ? part : part.charAt(0).toUpperCase() + part.slice(1)))
    .join("")
}

export function agentAccentClass(agent: string) {
  if (sameToken(agent, "explore")) return "border-[#83c5be] text-[#83c5be]"
  if (sameToken(agent, "frontend-agent") || sameToken(agent, "frontend")) return "border-[#f2b5d4] text-[#f2b5d4]"
  if (sameToken(agent, "general")) return "border-[#f2cc8f] text-[#f2cc8f]"
  if (sameToken(agent, "vlm-agent") || sameToken(agent, "vlm")) return "border-[#90dbf4] text-[#90dbf4]"
  if (sameToken(agent, "Plan")) return "border-[#8fcfb1] text-[#8fcfb1]"
  return "border-[#bda0ff] text-[#bda0ff]"
}

export function formatSeconds(ms: number) {
  return `${(ms / 1000).toFixed(1)}s`
}
