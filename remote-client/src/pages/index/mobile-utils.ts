import { createSignal, onCleanup, onMount } from "solid-js"

type NavigatorWithUserAgentData = Navigator & {
  userAgentData?: {
    mobile?: boolean
    getHighEntropyValues?: (hints: string[]) => Promise<{ mobile?: boolean }>
  }
}

const MOBILE_MAX_WIDTH = 900

const DEFAULT_HEIGHT_VAR = "--dvh"
const DEFAULT_KEYBOARD_VAR = "--keyboard-height"

export interface UseDynamicViewportOptions {
  /**
   * CSS custom property name for the visual viewport height.
   * Injected on `document.documentElement`.
   * @default '--dvh'
   */
  heightVar?: string
  /**
   * CSS custom property name for the on-screen keyboard height.
   * Injected on `document.documentElement`. `0px` when the keyboard is closed.
   * @default '--keyboard-height'
   */
  keyboardVar?: string
  /**
   * When false, the hook is a no-op and clears any previously written vars.
   * @default true
   */
  enabled?: boolean
}

export interface UseDynamicViewportResult {
  /** Current visual viewport height in pixels. */
  viewportHeight: () => number
  /** Current on-screen keyboard height in pixels (0 when closed). */
  keyboardHeight: () => number
  /** True when the on-screen keyboard is currently open. */
  isKeyboardOpen: () => boolean
}

/**
 * Solid port of `use-dynamic-viewport` (rl0425).
 *
 * Injects two CSS custom properties on `document.documentElement`:
 *   - `--dvh` (configurable): actual visible viewport height
 *   - `--keyboard-height` (configurable): keyboard height in px (0 when closed)
 *
 * Captures `window.innerHeight` / `innerWidth` at mount as a stable reference
 * before any keyboard interaction, then derives keyboard height from the
 * difference against `visualViewport.height`. We still track iOS
 * `visualViewport.offsetTop` because Safari pans the visual viewport during
 * focus; the layout uses that offset to cover the visible browser viewport.
 */
export function useDynamicViewport(
  options: UseDynamicViewportOptions = {}
): UseDynamicViewportResult {
  const {
    heightVar = DEFAULT_HEIGHT_VAR,
    keyboardVar = DEFAULT_KEYBOARD_VAR,
    enabled = true,
  } = options

  const [viewportHeight, setViewportHeight] = createSignal(0)
  const [keyboardHeight, setKeyboardHeight] = createSignal(0)

  const heightVarRef: { current: string } = { current: heightVar }
  const keyboardVarRef: { current: string } = { current: keyboardVar }
  heightVarRef.current = heightVar
  keyboardVarRef.current = keyboardVar

  onMount(() => {
    if (typeof window === "undefined" || !enabled) {
      document.documentElement.classList.remove("remote-keyboard-layout")
      document.documentElement.classList.remove("remote-input-focused")
      document.documentElement.classList.remove("remote-keyboard-open")
      document.documentElement.style.removeProperty(heightVarRef.current)
      document.documentElement.style.removeProperty(keyboardVarRef.current)
      document.documentElement.style.removeProperty("--visual-viewport-offset-top")
      document.documentElement.style.removeProperty("--visual-viewport-bottom")
      setViewportHeight(0)
      setKeyboardHeight(0)
      return
    }

    // Stable reference captured before any keyboard interaction
    const layoutHeightRef = window.innerHeight
    const layoutWidthRef = window.innerWidth

    document.documentElement.classList.add("remote-keyboard-layout")

    let rafId: number | null = null
    let pending = false

    const update = () => {
      const vv = window.visualViewport
      const visibleHeight = vv ? vv.height : window.innerHeight
      const kbHeight = Math.max(0, layoutHeightRef - visibleHeight)

      document.documentElement.style.setProperty(
        heightVarRef.current,
        `${Math.floor(visibleHeight)}px`
      )
      document.documentElement.style.setProperty(
        keyboardVarRef.current,
        `${Math.floor(kbHeight)}px`
      )
      document.documentElement.style.setProperty(
        "--visual-viewport-offset-top",
        `${Math.floor(vv?.offsetTop ?? 0)}px`
      )
      document.documentElement.style.setProperty(
        "--visual-viewport-bottom",
        `${Math.floor(visibleHeight + (vv?.offsetTop ?? 0))}px`
      )
      document.documentElement.classList.toggle("remote-keyboard-open", kbHeight > 120)

      setViewportHeight(visibleHeight)
      setKeyboardHeight(kbHeight)
    }

    const handleViewportChange = () => {
      const vv = window.visualViewport
      if (!vv) {
        if (pending) return
        pending = true
        rafId = window.requestAnimationFrame(() => {
          pending = false
          rafId = null
          update()
        })
        return
      }

      // Skip orientation/real resize; keyboard open should not change width.
      // Do not skip vv.offsetTop > 0: on iOS Safari this is the panned state we
      // need to track so the flex shell can fit the visible viewport immediately.
      if (window.innerWidth !== layoutWidthRef) return

      if (pending) return
      pending = true
      rafId = window.requestAnimationFrame(() => {
        pending = false
        rafId = null
        update()
      })
    }

    update()
    const vv = window.visualViewport
    const syncAfterFocus = () => {
      document.scrollingElement?.scrollTo({ top: 0, behavior: "auto" })
      handleViewportChange()
    }
    const targetIsTextInput = (target: EventTarget | null) =>
      target instanceof HTMLTextAreaElement || target instanceof HTMLInputElement
    const prepareForFocus = (event: Event) => {
      if (!isMobileViewport() || !targetIsTextInput(event.target)) return
      document.documentElement.classList.add("remote-input-focused")
      syncAfterFocus()
    }
    const correctIOSPan = (event: Event) => {
      if (!isMobileViewport() || !targetIsTextInput(event.target)) return
      document.documentElement.classList.add("remote-input-focused")
      // Safari pans after focus and again as the accessory bar settles. Correct a
      // few times, not continuously, so this behaves like the user's manual drag.
      window.requestAnimationFrame(syncAfterFocus)
      window.setTimeout(syncAfterFocus, 40)
      window.setTimeout(syncAfterFocus, 120)
      window.setTimeout(syncAfterFocus, 260)
      window.setTimeout(syncAfterFocus, 520)
    }
    const resetOuterScroll = () => {
      if (!isMobileViewport()) return
      document.scrollingElement?.scrollTo({ top: 0, behavior: "auto" })
      handleViewportChange()
    }
    const releaseIOSPan = () => {
      window.setTimeout(() => {
        if (document.activeElement instanceof HTMLTextAreaElement || document.activeElement instanceof HTMLInputElement) return
        document.documentElement.classList.remove("remote-input-focused")
        document.documentElement.classList.remove("remote-keyboard-open")
      }, 120)
    }

    vv?.addEventListener("resize", handleViewportChange)
    vv?.addEventListener("scroll", handleViewportChange)
    window.addEventListener("resize", handleViewportChange)
    window.addEventListener("orientationchange", handleViewportChange)
    window.addEventListener("popstate", resetOuterScroll)
    window.addEventListener("pageshow", resetOuterScroll)
    window.addEventListener("pagehide", resetOuterScroll)
    document.addEventListener("pointerdown", prepareForFocus, true)
    document.addEventListener("touchstart", prepareForFocus, true)
    document.addEventListener("focusin", correctIOSPan)
    document.addEventListener("focusout", releaseIOSPan)

    onCleanup(() => {
      if (rafId !== null) window.cancelAnimationFrame(rafId)
      vv?.removeEventListener("resize", handleViewportChange)
      vv?.removeEventListener("scroll", handleViewportChange)
      window.removeEventListener("resize", handleViewportChange)
      window.removeEventListener("orientationchange", handleViewportChange)
      window.removeEventListener("popstate", resetOuterScroll)
      window.removeEventListener("pageshow", resetOuterScroll)
      window.removeEventListener("pagehide", resetOuterScroll)
      document.removeEventListener("pointerdown", prepareForFocus, true)
      document.removeEventListener("touchstart", prepareForFocus, true)
      document.removeEventListener("focusin", correctIOSPan)
      document.removeEventListener("focusout", releaseIOSPan)
      document.documentElement.classList.remove("remote-keyboard-layout")
      document.documentElement.classList.remove("remote-input-focused")
      document.documentElement.classList.remove("remote-keyboard-open")
      document.documentElement.style.removeProperty(heightVarRef.current)
      document.documentElement.style.removeProperty(keyboardVarRef.current)
      document.documentElement.style.removeProperty("--visual-viewport-offset-top")
      document.documentElement.style.removeProperty("--visual-viewport-bottom")
      setViewportHeight(0)
      setKeyboardHeight(0)
    })
  })

  return {
    viewportHeight,
    keyboardHeight,
    isKeyboardOpen: () => keyboardHeight() > 0,
  }
}

// ============================================================================
// Mobile classification helpers
// ============================================================================

export function isMobileViewport() {
  if (typeof window === "undefined") return false
  return window.matchMedia(`(max-width: ${MOBILE_MAX_WIDTH}px)`).matches
}

/** Reactive `max-width: 900px` match — use to mount only one layout branch. */
export function useIsMobileViewport() {
  const [mobile, setMobile] = createSignal(isMobileViewport())

  onMount(() => {
    const mql = window.matchMedia(`(max-width: ${MOBILE_MAX_WIDTH}px)`)
    const onChange = () => setMobile(mql.matches)
    onChange()
    mql.addEventListener("change", onChange)
    onCleanup(() => mql.removeEventListener("change", onChange))
  })

  return mobile
}

export function resetMobileViewportScroll() {
  if (typeof document === "undefined" || !isMobileViewport()) return
  const vv = window.visualViewport
  const visibleHeight = vv?.height ?? window.innerHeight
  const viewportTop = vv?.offsetTop ?? 0
  const keyboardHeight = Math.max(0, window.innerHeight - visibleHeight)

  document.documentElement.classList.add("remote-keyboard-layout")
  document.scrollingElement?.scrollTo({ top: 0, behavior: "auto" })
  window.scrollTo({ top: 0, behavior: "auto" })
  document.documentElement.style.setProperty("--dvh", `${Math.floor(visibleHeight)}px`)
  document.documentElement.style.setProperty("--keyboard-height", `${Math.floor(keyboardHeight)}px`)
  document.documentElement.style.setProperty(
    "--visual-viewport-offset-top",
    `${Math.floor(viewportTop)}px`
  )
  document.documentElement.style.setProperty(
    "--visual-viewport-bottom",
    `${Math.floor(visibleHeight + viewportTop)}px`
  )
}

/**
 * Device class for composer Enter behavior (not viewport width).
 * Desktop-class devices: Enter submits. Phone/tablet-class: Enter inserts newline.
 */
export function isMobileDevice(): boolean {
  if (typeof navigator === "undefined") return false

  const uaData = (navigator as NavigatorWithUserAgentData).userAgentData
  if (uaData?.mobile === true) return true
  if (uaData?.mobile === false) return false

  const ua = navigator.userAgent

  if (/iPhone|iPod/i.test(ua)) return true
  if (/iPad/i.test(ua)) return true
  if (navigator.maxTouchPoints > 1 && /Macintosh|Mac OS X/i.test(ua)) return true

  if (/Android/i.test(ua)) return true

  if (/webOS|BlackBerry|IEMobile|Opera Mini|Windows Phone/i.test(ua)) return true

  return false
}

export function enterSubmitsPromptOnDevice(): boolean {
  return !isMobileDevice()
}

/** Resolves on mount; refines with Client Hints when available. */
export function useEnterSubmitsPrompt() {
  const [enterSubmits, setEnterSubmits] = createSignal(
    typeof navigator !== "undefined" ? enterSubmitsPromptOnDevice() : true
  )

  onMount(() => {
    setEnterSubmits(enterSubmitsPromptOnDevice())

    const uaData = (navigator as NavigatorWithUserAgentData).userAgentData
    if (uaData?.getHighEntropyValues) {
      void uaData
        .getHighEntropyValues(["mobile"])
        .then((values) => {
          if (typeof values.mobile === "boolean") {
            setEnterSubmits(!values.mobile)
          }
        })
        .catch(() => {})
    }
  })

  return enterSubmits
}

// ============================================================================
// Mobile layout (use-dynamic-viewport powered)
// ============================================================================

/**
 * Drive the mobile shell from `--dvh` (visual viewport height). The composer
 * stays in normal flex flow on mobile; we do not offset it by keyboard height
 * because the shell itself shrinks to the visual viewport.
 */
export function useMobileKeyboardLayout() {
  useDynamicViewport({ enabled: isMobileViewport() })
}

/** @deprecated Use useMobileKeyboardLayout. */
export function useVisualViewportOffset() {
  useMobileKeyboardLayout()
}