import {
  FileDiff,
  preloadHighlighter,
  processFile,
  type FileDiffMetadata,
  type FileDiffOptions,
} from "@pierre/diffs"
import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js"
import type { GitViewerController } from "./page-types"
import { cx } from "../../lib/cx"

export type DiffFile = NonNullable<ReturnType<GitViewerController["status"]>>["diff_files"][number]

let highlighterReady: Promise<void> | null = null

/** Keep Crabcode's chrome while leaving Pierre's responsive code grid intact. */
function pierreChromeCss(opts: { compact?: boolean; wrap?: boolean }): string {
  return `
    :host {
      --diffs-font-size: 12px;
      --diffs-line-height: 18px;
      --diffs-gap-block: 0px;
      --diffs-dark-bg: #0f0f0f;
      /* Kill Pierre's default 2px dark gutter seam. */
      --diffs-gap-style: 0 solid transparent;
      display: block;
      width: 100%;
      min-width: 0;
      max-width: 100%;
      ${opts.compact ? "max-height: min(28rem, 52vh); overflow: auto;" : ""}
    }

    /* Roomier file header (Pierre defaults to ~tight 10px padding). */
    [data-diffs-header],
    [data-diffs-header="default"],
    [data-file-info] {
      padding: 12px 14px !important;
      min-height: 2.5rem;
      box-sizing: border-box;
      align-items: center;
      gap: 0.5rem;
    }

    /* Clickable accordion affordance for collapsing a file's diff body. */
    [data-diffs-header],
    [data-diffs-header="default"] {
      font-size: 12px;
      cursor: pointer;
      user-select: none;
    }

    [data-diffs-header]:hover,
    [data-diffs-header="default"]:hover {
      filter: brightness(1.08);
    }

    [data-header-content],
    [data-title],
    [data-metadata] {
      line-height: 1.35;
    }

    /*
     * Put +/- in the gutter (number column), not in the code buffer.
     * We disable Pierre's classic indicators (which paint on [data-line]) and
     * render our own markers on [data-column-number] instead.
     *
     * Reserve a fixed 2ch marker slot on EVERY number cell so context lines
     * keep the same right-edge alignment as addition/deletion lines.
     * (Otherwise " 822 -" is wider than "822" and numbers look shifted.)
     */
    [data-gutter-buffer],
    [data-column-number] {
      border-right: 0 !important;
      position: relative;
      /* number + one space + marker */
      padding-inline-end: 2ch !important;
      box-sizing: border-box;
    }

    [data-column-number][data-line-type="change-addition"]::after,
    [data-column-number][data-line-type="change-deletion"]::after {
      position: absolute;
      top: 0;
      right: 0;
      width: 2ch;
      height: 1lh;
      display: inline-flex;
      align-items: center;
      justify-content: flex-start;
      font-weight: 700;
      user-select: none;
      pointer-events: none;
      white-space: pre;
      box-sizing: border-box;
    }

    /* Leading space keeps one gap between the number and the marker. */
    [data-column-number][data-line-type="change-addition"]::after {
      content: " +";
      color: var(--diffs-addition-base);
    }

    [data-column-number][data-line-type="change-deletion"]::after {
      content: " -";
      color: var(--diffs-deletion-base);
    }

    /* Code lines should not reserve classic-indicator gutter space. */
    [data-line],
    [data-no-newline] {
      padding-inline-start: 1ch !important;
    }

    [data-line]::before,
    [data-no-newline]::before {
      content: none !important;
      display: none !important;
    }

    ${
      opts.wrap
        ? `
    /* Wrap mode: full width, no horizontal scroll. */
    [data-diff],
    [data-file] {
      --diffs-code-grid: var(--diffs-grid-number-column-width) minmax(0, 1fr);
    }

    [data-overflow="wrap"] [data-code] {
      width: 100%;
      min-width: 0;
      column-gap: 0 !important;
    }

    [data-overflow="wrap"] [data-line],
    [data-overflow="wrap"] [data-content] {
      min-width: 0;
      width: 100%;
      white-space: pre-wrap;
      overflow-wrap: anywhere;
      word-break: break-word;
    }
    `
        : `
    /* Pierre owns this scroll grid. Replacing both tracks with max-content can
     * strand a narrow card on its sticky number gutter while highlighted code
     * sits outside the initial viewport. */
    [data-overflow="scroll"] [data-code] {
      min-width: 0;
      max-width: 100%;
      column-gap: 0 !important;
    }

    [data-overflow="scroll"] [data-content] {
      min-width: 0;
    }
    `
    }
  `
}

function ensureHighlighter() {
  // Use Shiki language ids only — "plaintext" is invalid and aborts preload.
  if (!highlighterReady) {
    highlighterReady = preloadHighlighter({
      themes: ["pierre-dark", "pierre-light"],
      langs: [
        "text",
        "typescript",
        "tsx",
        "javascript",
        "jsx",
        "json",
        "rust",
        "python",
        "go",
        "css",
        "html",
        "markdown",
        "yaml",
        "toml",
        "bash",
        "shellscript",
        "diff",
      ],
    })
      .then(() => undefined)
      .catch((error) => {
        highlighterReady = null
        throw error
      })
  }
  return highlighterReady
}

/**
 * Rebuild a git-style unified patch from remote-parsed lines.
 * Server stores hunk headers as full `@@ … @@` and strips +/-/space prefixes from body lines.
 */
export function toUnifiedPatch(file: DiffFile): string {
  const oldPath = file.old_path ?? file.path
  const lines: string[] = [
    `diff --git a/${oldPath} b/${file.path}`,
    `--- a/${oldPath}`,
    `+++ b/${file.path}`,
  ]
  for (const line of file.lines) {
    if (line.kind === "hunk") {
      lines.push(line.text.startsWith("@@") ? line.text : `@@ ${line.text}`)
      continue
    }
    if (line.kind === "meta") {
      lines.push(line.text)
      continue
    }
    if (line.kind === "add") {
      lines.push(`+${line.text}`)
      continue
    }
    if (line.kind === "remove") {
      lines.push(`-${line.text}`)
      continue
    }
    lines.push(` ${line.text}`)
  }
  return `${lines.join("\n")}\n`
}

function parseFileDiff(file: DiffFile): FileDiffMetadata | undefined {
  try {
    return processFile(toUnifiedPatch(file), {
      isGitDiff: true,
      cacheKey: `${file.path}:${file.additions}:${file.deletions}:${file.lines.length}`,
      throwOnError: false,
    })
  } catch (error) {
    console.error("Failed to parse git patch for diffs viewer", error)
    return undefined
  }
}

function pierreOptions(opts: {
  compact?: boolean
  wrap?: boolean
  collapsed?: boolean
}): FileDiffOptions<undefined> {
  return {
    theme: { dark: "pierre-dark", light: "pierre-light" },
    themeType: "dark",
    diffStyle: "unified",
    overflow: opts.wrap ? "wrap" : "scroll",
    disableFileHeader: false,
    stickyHeader: true,
    expandUnchanged: false,
    hunkSeparators: "line-info",
    expansionLineCount: opts.compact ? 10 : 20,
    collapsedContextThreshold: opts.compact ? 4 : 6,
    // Classic paints +/- on the code line; we render markers in the gutter instead.
    diffIndicators: "none",
    lineDiffType: "word-alt",
    disableLineNumbers: false,
    disableBackground: false,
    collapsed: opts.collapsed ?? false,
    unsafeCSS: pierreChromeCss(opts),
  }
}

function findPierreHeader(root: HTMLElement): HTMLElement | null {
  const host = root.matches?.("diffs-container")
    ? root
    : (root.querySelector("diffs-container") as HTMLElement | null)
  const shadow = host?.shadowRoot
  if (!shadow) return null
  return (
    (shadow.querySelector("[data-diffs-header]") as HTMLElement | null) ??
    (shadow.querySelector("[data-file-header]") as HTMLElement | null) ??
    (shadow.querySelector("header") as HTMLElement | null)
  )
}

function hasRenderedDiffLines(root: HTMLElement, collapsed: boolean): boolean {
  const host = root.matches?.("diffs-container")
    ? root
    : (root.querySelector("diffs-container") as HTMLElement | null)
  const shadow = host?.shadowRoot
  if (!shadow) return false
  if (collapsed) return findPierreHeader(root) !== null
  // onPostRender also fires for Pierre's placeholder pass. Real highlighted
  // output has non-empty code content; placeholder rows can already have line
  // nodes but do not contain source text yet.
  const content = shadow.querySelector("[data-content]")
  const line = content?.querySelector("[data-line], [data-no-newline], [data-line-annotation]")
  return line !== null && (content?.textContent ?? "").replaceAll(" ", "").trim().length > 0
}

function hashText(hash: number, value: string): number {
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

/** Stable across git status polls when this file's rendered content is unchanged. */
function diffFileRenderKey(file: DiffFile): string {
  let hash = hashText(
    2166136261,
    [file.path, file.old_path ?? "", file.status, file.binary, file.truncated].join("|")
  )
  for (const line of file.lines) {
    hash = hashText(
      hash,
      [line.kind, line.old_line ?? "", line.new_line ?? "", line.text].join("|")
    )
  }
  return `${file.path}:${file.lines.length}:${hash.toString(36)}`
}

/**
 * Keep Pierre cards hidden until all have completed their first real render.
 * This prevents its internal loading bars and early truncation badges from
 * flashing as a stack of malformed-looking cards.
 */
export function GitDiffBatch(props: {
  files: DiffFile[]
  compact?: boolean
  wrap?: boolean
}) {
  const entries = createMemo<{ file: DiffFile; key: string }[]>((previous = []) => {
    const mounted = new Map(previous.map((entry) => [entry.key, entry]))
    return props.files.map((file, index) => {
      const key = `${index}:${diffFileRenderKey(file)}`
      // Preserve object identity so Solid's <For> retains the already-rendered
      // Pierre instance when polling returns equivalent diff content.
      return mounted.get(key) ?? { file, key }
    })
  })
  const [readyKeys, setReadyKeys] = createSignal<ReadonlySet<string>>(new Set())
  const [revealedBatchKey, setRevealedBatchKey] = createSignal<string | null>(null)
  const batchKey = createMemo(() => entries().map((entry) => entry.key).join("\n"))

  createEffect(() => {
    const valid = new Set(entries().map((entry) => entry.key))
    setReadyKeys((current) => new Set([...current].filter((key) => valid.has(key))))
  })

  const allReady = createMemo(() => {
    const ready = readyKeys()
    const current = entries()
    return current.length === 0 || current.every((entry) => ready.has(entry.key))
  })

  // Reveal is tied to the exact content key. When status data arrives or changes,
  // the key mismatch hides cards synchronously—there is no effect-sized frame in
  // which new truncation badges can appear under an old `revealed = true` value.
  const revealed = () =>
    allReady() && batchKey().length > 0 && revealedBatchKey() === batchKey()

  // Once every Pierre instance is ready, wait two paint frames before exposing
  // the batch so shadow-DOM CSS and syntax token styles have settled too.
  createEffect(() => {
    if (!allReady()) {
      return
    }
    const key = batchKey()
    if (!key) return
    let secondFrame = 0
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        if (batchKey() === key && allReady()) setRevealedBatchKey(key)
      })
    })
    onCleanup(() => {
      cancelAnimationFrame(firstFrame)
      if (secondFrame) cancelAnimationFrame(secondFrame)
    })
  })

  const onReady = (key: string) => {
    setReadyKeys((current) => {
      if (current.has(key)) return current
      const next = new Set(current)
      next.add(key)
      return next
    })
  }

  return (
    <div class="relative min-h-[3.5rem] min-w-0">
      <Show when={!revealed()}>
        <div
          class="absolute inset-x-0 top-0 flex min-h-[3.5rem] items-center gap-2 rounded-md border border-[var(--line)] bg-white/[0.018] px-3 text-[0.72rem] text-[var(--faint)]"
          role="status"
          aria-live="polite"
        >
          <span class="size-3 animate-spin rounded-full border border-white/15 border-t-[var(--muted)]" />
          <span>Preparing highlighted diffs…</span>
        </div>
      </Show>

      <div
        class={cx(
          "grid min-w-0 gap-2",
          !revealed() && "pointer-events-none invisible absolute inset-x-0 top-0"
        )}
        aria-hidden={!revealed()}
      >
        <For each={entries()}>
          {(entry) => (
            <GitDiffFile
              file={entry.file}
              compact={props.compact}
              wrap={props.wrap}
              onReady={() => onReady(entry.key)}
            />
          )}
        </For>
      </div>
    </div>
  )
}

function wireHeaderAccordion(
  root: HTMLElement,
  isCollapsed: () => boolean,
  toggleCollapsed: () => void
) {
  const header = findPierreHeader(root)
  if (!header) return

  header.style.cursor = "pointer"
  header.setAttribute("role", "button")
  header.setAttribute("tabindex", "0")
  header.setAttribute("aria-expanded", isCollapsed() ? "false" : "true")
  header.setAttribute("title", isCollapsed() ? "Expand file diff" : "Collapse file diff")

  if (header.dataset.accordionWired === "1") return
  header.dataset.accordionWired = "1"

  const onActivate = (event: Event) => {
    // Ignore clicks on nested interactive controls (copy buttons, etc.).
    const target = event.target as HTMLElement | null
    if (target && target !== header && target.closest("button, a, input, select, textarea")) return
    event.preventDefault()
    toggleCollapsed()
  }

  header.addEventListener("click", onActivate)
  header.addEventListener("keydown", (event: KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault()
      toggleCollapsed()
    }
  })
}

export function GitDiffFile(props: {
  file: DiffFile
  compact?: boolean
  /** Soft-wrap long lines instead of horizontal scroll. */
  wrap?: boolean
  /** Called once Pierre has rendered real content or a terminal fallback. */
  onReady?: () => void
}) {
  const [host, setHost] = createSignal<HTMLDivElement | undefined>()
  const [failed, setFailed] = createSignal(false)
  const [collapsed, setCollapsed] = createSignal(false)

  onMount(() => {
    void ensureHighlighter().catch((error) => {
      console.error("Failed to preload diffs highlighter", error)
    })
  })

  // Expanding/collapsing is per-file instance; reset when the path changes.
  createEffect(() => {
    props.file.path
    setCollapsed(false)
  })

  createEffect(() => {
    const el = host()
    const file = props.file
    const compact = props.compact
    const wrap = props.wrap
    const isCollapsed = collapsed()
    setFailed(false)
    if (!el) return

    el.replaceChildren()
    if (file.binary || file.lines.length === 0) {
      props.onReady?.()
      return
    }

    let cancelled = false
    let diff: FileDiff | undefined

    const run = async () => {
      try {
        await ensureHighlighter()
      } catch (error) {
        console.error("Failed to preload diffs highlighter", error)
      }
      if (cancelled || host() !== el) return

      const fileDiff = parseFileDiff(file)
      if (!fileDiff) {
        if (!cancelled) {
          setFailed(true)
          props.onReady?.()
        }
        return
      }

      diff = new FileDiff(
        {
          ...pierreOptions({ compact, wrap, collapsed: isCollapsed }),
          onPostRender: (node) => {
            wireHeaderAccordion(
              node,
              () => collapsed(),
              () => setCollapsed((value) => !value)
            )
            if (hasRenderedDiffLines(node, isCollapsed)) props.onReady?.()
          },
        },
        undefined,
        false
      )

      try {
        const ok = diff.render({
          fileDiff,
          containerWrapper: el,
        })
        // Header may already exist even if render returns false in edge cases.
        wireHeaderAccordion(
          el,
          () => collapsed(),
          () => setCollapsed((value) => !value)
        )
        if (hasRenderedDiffLines(el, isCollapsed)) props.onReady?.()
        if (!ok) {
          window.setTimeout(() => {
            if (cancelled || host() !== el) return
            const container = el.querySelector("diffs-container")
            const shadow = container?.shadowRoot
            const hasContent = Boolean(
              shadow?.querySelector("[data-diff], [data-file], pre, [data-line], [data-diffs-header]") ||
                (container && container.childElementCount > 0)
            )
            if (!hasContent) {
              setFailed(true)
              props.onReady?.()
            } else if (hasRenderedDiffLines(el, isCollapsed)) {
              props.onReady?.()
            }
          }, 1500)
        }
      } catch (error) {
        console.error("Failed to render git diff", error)
        if (!cancelled) {
          setFailed(true)
          props.onReady?.()
        }
      }
    }

    void run()

    onCleanup(() => {
      cancelled = true
      try {
        diff?.cleanUp()
      } catch {
        // ignore
      }
      if (host() === el) el.replaceChildren()
    })
  })

  return (
    <article class="w-full min-w-0 max-w-full overflow-hidden rounded-md border border-[var(--line)] bg-black/12">
      <Show
        when={!props.file.binary && props.file.lines.length > 0}
        fallback={
          <div class="px-2 py-2 text-[0.72rem] text-[var(--faint)]">
            <Show when={props.file.binary} fallback="No textual diff preview.">
              Binary file changed.
            </Show>
            <div class="mt-1 font-mono text-[0.68rem] text-[var(--muted)]">{props.file.path}</div>
          </div>
        }
      >
        <Show when={failed()}>
          <button
            type="button"
            class="flex w-full items-center justify-between gap-2 border-b border-[var(--line)] px-2 py-1.5 text-left font-mono text-[0.7rem] text-[var(--muted)]"
            onClick={() => setCollapsed((value) => !value)}
          >
            <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
              {props.file.old_path ? `${props.file.old_path} → ${props.file.path}` : props.file.path}
            </span>
            <span class="shrink-0">
              <span class="text-[#6ecf9a]">+{props.file.additions}</span>
              <span class="ml-1 text-[#f08a96]">−{props.file.deletions}</span>
            </span>
          </button>
          <Show when={!collapsed()}>
            <pre
              class={cx(
                "m-0 max-h-[min(22rem,42vh)] overflow-auto px-2 py-2 font-mono text-[0.68rem] leading-[1.4] text-[var(--muted)]",
                props.wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"
              )}
            >
              {toUnifiedPatch(props.file)}
            </pre>
          </Show>
        </Show>
        <div
          ref={setHost}
          class={cx(
            "pierre-diff-host block w-full min-w-0 max-w-full bg-[#0f0f0f]",
            "[&_diffs-container]:block [&_diffs-container]:w-full [&_diffs-container]:min-w-0 [&_diffs-container]:max-w-full",
            failed() && "hidden"
          )}
        />
        <Show when={props.file.truncated && !collapsed()}>
          <div class="border-t border-[var(--line)] px-2 py-1 text-[0.68rem] text-[#d4bc82]">… truncated</div>
        </Show>
      </Show>
    </article>
  )
}
