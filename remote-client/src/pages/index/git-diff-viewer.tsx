import {
  FileDiff,
  preloadHighlighter,
  processFile,
  type FileDiffMetadata,
  type FileDiffOptions,
} from "@pierre/diffs"
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js"
import type { GitViewerController } from "./page-types"
import { cx } from "../../lib/cx"

type DiffFile = NonNullable<ReturnType<GitViewerController["status"]>>["diff_files"][number]

let highlighterReady: Promise<void> | null = null

/**
 * Pierre paints line backgrounds on grid tracks sized with `1fr`, so red/green
 * only spans the viewport. When the user scrolls horizontally the color stops.
 *
 * Important: do NOT set width:100% on line rows — that collapses the track back
 * to the viewport. Use min-width:max(100%, max-content) on content cells only.
 */
function pierreChromeCss(opts: { compact?: boolean; wrap?: boolean }): string {
  const overflow = opts.wrap ? "wrap" : "scroll"
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
      overflow === "scroll"
        ? `
    /* Grow the content track past the viewport when lines are long. */
    [data-diff],
    [data-file] {
      --diffs-code-grid: var(--diffs-grid-number-column-width) minmax(max-content, 100%);
    }

    [data-overflow="scroll"] [data-code] {
      grid-template-columns: var(--diffs-code-grid) !important;
      width: max-content;
      min-width: 100%;
      column-gap: 0 !important;
    }

    /*
     * Fill the grown track without forcing the track width back to 100%.
     * width:100% was collapsing max-content → viewport and re-breaking backgrounds.
     */
    [data-overflow="scroll"] [data-content],
    [data-overflow="scroll"] [data-line],
    [data-overflow="scroll"] [data-no-newline],
    [data-overflow="scroll"] [data-line-annotation],
    [data-overflow="scroll"] [data-merge-conflict],
    [data-overflow="scroll"] [data-merge-conflict-actions] {
      min-width: 100%;
      width: auto;
    }

    /* Keep sticky gutter above scrolled code; keep red/green number paint intact. */
    [data-overflow="scroll"] [data-gutter] {
      z-index: 4;
    }
    `
        : `
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
    if (file.binary || file.lines.length === 0) return

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
        if (!cancelled) setFailed(true)
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
        if (!ok) {
          window.setTimeout(() => {
            if (cancelled || host() !== el) return
            const container = el.querySelector("diffs-container")
            const shadow = container?.shadowRoot
            const hasContent = Boolean(
              shadow?.querySelector("[data-diff], [data-file], pre, [data-line], [data-diffs-header]") ||
                (container && container.childElementCount > 0)
            )
            if (!hasContent) setFailed(true)
          }, 1500)
        }
      } catch (error) {
        console.error("Failed to render git diff", error)
        if (!cancelled) setFailed(true)
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
