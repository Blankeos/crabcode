import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  type Accessor,
} from "solid-js"
import type { RemoteMessage } from "../../remote-api"
import type { ThreadItem } from "./page-types"
import { cx } from "../../lib/cx"

export type MessageMarker = {
  id: string
  preview: string
  role: string
}

export function messageDomId(messageIndex: number): string {
  return `msg-${messageIndex}`
}

function messagePreviewText(message: RemoteMessage): string {
  return (message.content?.trim() || message.reasoning?.trim() || "").replace(/\s+/g, " ")
}

/** Skip empty / interrupted shells that would only show a useless "Reply" preview. */
export function isRailMessage(message: RemoteMessage): boolean {
  if (message.role !== "user" && message.role !== "assistant") return false
  return messagePreviewText(message).length > 0
}

function previewFromMessage(message: RemoteMessage): string {
  const text = messagePreviewText(message)
  return text.length > 120 ? `${text.slice(0, 117)}…` : text
}

/**
 * One dash per non-empty user/assistant message as rendered in the thread.
 * Must use threadItems (not raw visibleMessages) so ids match DOM anchors —
 * assistant turns with tools are split into new message objects.
 */
export function markersFromThreadItems(items: ThreadItem[]): MessageMarker[] {
  const markers: MessageMarker[] = []
  let index = 0
  for (const item of items) {
    if (item.type !== "message" || !isRailMessage(item.message)) continue
    markers.push({
      id: messageDomId(index),
      role: item.message.role,
      preview: previewFromMessage(item.message),
    })
    index += 1
  }
  return markers
}

/** Map each rail-eligible thread message object → stable msg-N id. */
export function messageDomIdsFromThreadItems(items: ThreadItem[]): Map<object, string> {
  const ids = new Map<object, string>()
  let index = 0
  for (const item of items) {
    if (item.type !== "message" || !isRailMessage(item.message)) continue
    ids.set(item.message, messageDomId(index))
    index += 1
  }
  return ids
}

function messageOffsetTop(scrollEl: HTMLElement, el: HTMLElement): number {
  // offsetTop chain is more stable than getBoundingClientRect mid-animation.
  let top = 0
  let node: HTMLElement | null = el
  while (node && node !== scrollEl) {
    top += node.offsetTop
    node = node.offsetParent as HTMLElement | null
  }
  // Fallback if offsetParent chain doesn't reach the scroll container.
  if (!node) {
    const scrollRect = scrollEl.getBoundingClientRect()
    const elRect = el.getBoundingClientRect()
    return elRect.top - scrollRect.top + scrollEl.scrollTop
  }
  return top
}

/**
 * Cancellable smooth scroll.
 * Generation token so stale rAF frames never write after a retarget.
 * Long distances stay capped so retargets remain snappy.
 */
function createInterruptibleScroller() {
  let frame = 0
  let generation = 0
  let pinnedId: string | null = null

  const cancel = () => {
    generation += 1
    if (frame) {
      cancelAnimationFrame(frame)
      frame = 0
    }
    pinnedId = null
  }

  const isAnimating = () => frame !== 0
  const getPinnedId = () => pinnedId

  const scrollTo = (
    scrollEl: HTMLElement,
    top: number,
    opts?: { pinId?: string; onDone?: () => void }
  ) => {
    generation += 1
    const gen = generation
    if (frame) {
      cancelAnimationFrame(frame)
      frame = 0
    }
    pinnedId = opts?.pinId ?? null

    const start = scrollEl.scrollTop
    const maxTop = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight)
    const end = Math.min(maxTop, Math.max(0, top))
    const delta = end - start
    if (Math.abs(delta) < 1) {
      scrollEl.scrollTop = end
      frame = 0
      pinnedId = null
      opts?.onDone?.()
      return
    }

    // Cap duration so long-session jumps don't feel stuck; still smooth.
    const durationMs = Math.min(520, Math.max(160, Math.abs(delta) * 0.35))
    const startedAt = performance.now()
    const ease = (t: number) => 1 - (1 - t) ** 3

    const step = (now: number) => {
      if (gen !== generation) return
      const t = Math.min(1, (now - startedAt) / durationMs)
      scrollEl.scrollTop = start + delta * ease(t)
      if (t < 1) {
        frame = requestAnimationFrame(step)
        return
      }
      // Snap to final target (layout may have shifted slightly).
      scrollEl.scrollTop = end
      frame = 0
      pinnedId = null
      opts?.onDone?.()
    }
    frame = requestAnimationFrame(step)
  }

  return { cancel, scrollTo, isAnimating, getPinnedId }
}

function scrollTargetTop(scrollEl: HTMLElement, messageId: string): number | null {
  const target = scrollEl.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(messageId)}"]`)
  if (!target) return null
  return Math.max(0, messageOffsetTop(scrollEl, target) - 12)
}

/** Single-pass active marker from cached elements (O(n), not O(n²) querySelector). */
function findActiveMarkerId(scrollEl: HTMLElement, markers: MessageMarker[]): string | null {
  if (markers.length === 0) return null

  if (scrollEl.scrollTop <= 8) return markers[0]?.id ?? null

  const maxScroll = scrollEl.scrollHeight - scrollEl.clientHeight
  if (maxScroll > 0 && scrollEl.scrollTop >= maxScroll - 8) {
    return markers[markers.length - 1]?.id ?? null
  }

  const focusY = scrollEl.scrollTop + Math.min(72, scrollEl.clientHeight * 0.12)
  const nodes = scrollEl.querySelectorAll<HTMLElement>("[data-message-id]")
  const byId = new Map<string, HTMLElement>()
  for (const node of nodes) {
    const id = node.getAttribute("data-message-id")
    if (id) byId.set(id, node)
  }

  let active: string | null = markers[0]?.id ?? null
  for (const marker of markers) {
    const el = byId.get(marker.id)
    if (!el) continue
    if (messageOffsetTop(scrollEl, el) <= focusY) active = marker.id
    else break
  }
  return active
}

/**
 * Fixed right-edge message rail:
 * dim dashes per message, brighter/wider when active or hovered, hover preview.
 *
 * Vertically centers in the visible chat band (above the floating composer),
 * not the full thread viewport — otherwise the dock makes it look low.
 */
export function MessageMarkerRail(props: {
  threadItems: Accessor<ThreadItem[]>
  scrollEl: Accessor<HTMLElement | null | undefined>
  hidden?: Accessor<boolean>
  /** Bottom inset matching the composer dock / thread bottom padding. */
  bottomInset?: Accessor<string | undefined>
  /** Suppress stick-to-bottom while a rail jump owns scroll. */
  setNavigationLock?: (locked: boolean) => void
}) {
  const markers = createMemo(() => markersFromThreadItems(props.threadItems()))
  const [activeId, setActiveId] = createSignal<string | null>(null)
  const [hoveredId, setHoveredId] = createSignal<string | null>(null)
  const scroller = createInterruptibleScroller()

  // Only rebind listeners when the scroll element changes — NOT when markers()
  // updates (that was cancelling in-flight jumps mid 2↔7 spam).
  createEffect(() => {
    const scrollEl = props.scrollEl()
    if (!scrollEl) {
      setActiveId(null)
      return
    }

    const update = () => {
      const list = markers()
      if (list.length === 0) {
        setActiveId(null)
        return
      }
      // While a rail jump is in flight, keep the pinned marker active —
      // mid-scroll position would otherwise thrash active across 2↔7.
      const pinned = scroller.getPinnedId()
      if (pinned) {
        setActiveId(pinned)
        return
      }
      setActiveId(findActiveMarkerId(scrollEl, list))
    }
    update()
    // Manual wheel/touch should cancel an in-flight rail animation.
    const onUserScroll = () => {
      if (!scroller.isAnimating()) return
      scroller.cancel()
      props.setNavigationLock?.(false)
      setActiveId(findActiveMarkerId(scrollEl, markers()))
    }
    scrollEl.addEventListener("scroll", update, { passive: true })
    scrollEl.addEventListener("wheel", onUserScroll, { passive: true })
    scrollEl.addEventListener("touchstart", onUserScroll, { passive: true })
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null
    ro?.observe(scrollEl)
    onCleanup(() => {
      scroller.cancel()
      props.setNavigationLock?.(false)
      scrollEl.removeEventListener("scroll", update)
      scrollEl.removeEventListener("wheel", onUserScroll)
      scrollEl.removeEventListener("touchstart", onUserScroll)
      ro?.disconnect()
    })
  })

  const jumpTo = (messageId: string) => {
    const scrollEl = props.scrollEl()
    if (!scrollEl) return
    const top = scrollTargetTop(scrollEl, messageId)
    // Missing DOM target (stale id) — don't pin active without scrolling.
    if (top == null) {
      setActiveId(findActiveMarkerId(scrollEl, markers()))
      return
    }
    setActiveId(messageId)
    props.setNavigationLock?.(true)
    scroller.scrollTo(scrollEl, top, {
      pinId: messageId,
      onDone: () => {
        props.setNavigationLock?.(false)
        // Re-resolve in case layout shifted during the jump.
        const settled = scrollTargetTop(scrollEl, messageId)
        if (settled != null && Math.abs(scrollEl.scrollTop - settled) > 2) {
          scrollEl.scrollTop = settled
        }
        setActiveId(findActiveMarkerId(scrollEl, markers()))
      },
    })
  }

  return (
    <Show when={!props.hidden?.() && markers().length > 0}>
      <nav
        class="pointer-events-none absolute top-0 right-0 z-30 hidden w-12 md:flex"
        style={{ bottom: props.bottomInset?.() || "0px" }}
        aria-label="Message markers"
      >
        {/* Centered in the band above the composer (nav bottom inset), not full viewport. */}
        <div class="pointer-events-auto absolute top-1/2 right-2 flex max-h-full -translate-y-1/2 flex-col items-end overflow-visible py-3">
          <For each={markers()}>
            {(marker) => {
              const active = () => activeId() === marker.id
              const hovered = () => hoveredId() === marker.id
              const expanded = () => active() || hovered()
              return (
                <div class="relative flex items-center justify-end">
                  <button
                    type="button"
                    class={cx(
                      // Full padding is the hit target — thin dash is only visual.
                      "group flex h-4 w-10 items-center justify-end rounded-sm px-1 outline-none transition",
                      "focus-visible:ring-1 focus-visible:ring-white/30"
                    )}
                    aria-label={`Jump to ${marker.role} message: ${marker.preview}`}
                    aria-current={active() ? "true" : undefined}
                    onMouseEnter={() => setHoveredId(marker.id)}
                    onMouseLeave={() => setHoveredId((id) => (id === marker.id ? null : id))}
                    onFocus={() => setHoveredId(marker.id)}
                    onBlur={() => setHoveredId((id) => (id === marker.id ? null : id))}
                    onClick={() => jumpTo(marker.id)}
                  >
                    <span
                      class={cx(
                        "block h-[2px] rounded-full transition-[width,background-color] duration-150 ease-out",
                        expanded()
                          ? "w-5 bg-white/90"
                          : "w-2.5 bg-white/28 group-hover:w-5 group-hover:bg-white/70"
                      )}
                    />
                  </button>
                  <Show when={hovered()}>
                    <div
                      class={cx(
                        "pointer-events-none absolute right-full top-1/2 z-30 mr-2.5 w-[16rem]",
                        "-translate-y-1/2 origin-right rounded-xl px-3.5 py-2.5 text-left",
                        "shadow-[0_0.6rem_1.6rem_rgba(0,0,0,0.4)] backdrop-blur-md",
                        marker.role === "user"
                          ? "border border-[var(--brand-primary)]/35 bg-[var(--brand-primary)]"
                          : "border border-white/[0.12] bg-[#2a2a2a]/96"
                      )}
                      role="tooltip"
                    >
                      <Show when={marker.role === "assistant"}>
                        <div class="mb-1 text-[0.68rem] font-medium tracking-[0.06em] text-white/50">
                          Crabcode
                        </div>
                      </Show>
                      <span
                        class={cx(
                          "line-clamp-3 break-words text-[0.9rem] font-normal leading-relaxed",
                          marker.role === "user" ? "text-white" : "text-[#f0eee8]"
                        )}
                      >
                        {marker.preview}
                      </span>
                    </div>
                  </Show>
                </div>
              )
            }}
          </For>
        </div>
      </nav>
    </Show>
  )
}
