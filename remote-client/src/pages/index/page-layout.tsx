import { For, Index, Show, createMemo, createSignal } from "solid-js"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "cmdk-solid"
import { useIsMobileViewport } from "./mobile-utils"
import { IconBrainGlyph, IconClockCounterClockwise, IconF7ChevronDownSquare, IconGitDiff } from "../../assets/icons"
import { FadedEdgeEffect } from "../../components/remote/faded-edge-effect"
import { ProjectFavicon } from "../../components/remote/project-favicon"
import { ProjectList } from "../../components/remote/project-list"
import { Popover, PopoverContent, PopoverTrigger } from "../../components/ui/popover"
import { Resizable, ResizableHandle, ResizablePanel } from "../../components/ui/resizable"
import { IconArrowLeft, IconCaretDown, IconCheck, IconDots, IconFolder, IconPlus, IconSearch, IconServers, IconSidebar, IconX } from "../../icons"
import { mostRecentSession } from "./projects"
import { cx } from "../../lib/cx"
import { ICON_BUTTON, INPUT_BASE, PANEL_BASE, POPOVER_ANIMATION } from "./page-constants"
import type { CommandPaletteController, GitViewerController, HeaderController, PairPanelController, ProjectPathFormController, ProjectPickerController, RemoteClientUi, ServerPanelController, SidebarController, ThreadController } from "./page-types"
import { ComposerDock } from "./composer-dock"
import { EmptyThread } from "./empty-thread"
import { QuestionRequestPanel, PermissionRequestPanel } from "./request-panels"
import { SubagentSessionFooter } from "./subagent-footer"
import { ImagePreviewDialog, ThreadItemView } from "./thread-view"
import { ThreadTabsBar } from "./thread-tabs"
import { GitDiffFile } from "./git-diff-viewer"
import { isActiveServer } from "./server-utils"
import { relativeTime } from "./shared-utils"

export function RemoteClientPage(props: { ui: RemoteClientUi }) {
  const ui = props.ui
  const git = ui.header.gitViewer
  const isMobileViewport = useIsMobileViewport()
  const gitPanelOpen = createMemo(() => Boolean(git.open() && git.summary()?.is_repo))
  const mainColumn = () => (
    <main class="relative flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-[#171717]">
      <MainHeader header={ui.header} />
      <ThreadTabsBar tabs={ui.thread.tabs} />
      <ThreadViewport thread={ui.thread} />
      <Show when={!ui.thread.isSubagentView()} fallback={<SubagentSessionFooter footer={ui.subagentFooter} />}>
        <div class={cx(ui.thread.shellLoading() && "pointer-events-none opacity-55")}>
          <ComposerDock composer={ui.composer} />
        </div>
      </Show>
    </main>
  )

  const mainLayout = () => (
    <>
      {mainColumn()}

      <Show when={gitPanelOpen()}>
        <button
          class="fixed inset-0 z-[75] bg-black/45"
          type="button"
          aria-label="Close git changes"
          onClick={() => git.onOpenChange(false)}
        />
        <GitSidePanel git={git} variant="mobile" />
      </Show>
    </>
  )

  return (
    <div
      class={cx(
        "remote-mobile-root grid h-dvh overflow-hidden bg-[var(--bg)] max-[900px]:h-[var(--dvh,100dvh)] max-[900px]:max-h-[var(--dvh,100dvh)] max-[900px]:min-h-0 max-[900px]:grid-cols-1",
        "min-[901px]:grid-cols-[clamp(16.5rem,19vw,20rem)_minmax(0,1fr)]"
      )}
      style={ui.themeStyle()}
    >
      <PairOverlay pair={ui.pair} />
      <RemoteSidebar sidebar={ui.sidebar} />
      <button
        class={cx(
          "fixed inset-0 z-[70] hidden bg-black/45 max-[900px]:block",
          ui.sidebar.open() ? "max-[900px]:block" : "max-[900px]:hidden"
        )}
        type="button"
        onClick={() => ui.sidebar.setOpen(false)}
      />

      <div class="relative min-h-0 min-w-0 h-dvh max-[900px]:h-[var(--dvh,100dvh)] max-[900px]:max-h-[var(--dvh,100dvh)]">
        <Show
          when={gitPanelOpen()}
          fallback={
            <div class="h-full min-h-0 min-w-0 max-[900px]:h-[var(--dvh,100dvh)] max-[900px]:max-h-[var(--dvh,100dvh)]">
              {mainLayout()}
            </div>
          }
        >
          {/* One layout only — dual CSS-hidden trees double-mount ServerPopover and race open state. */}
          <Show
            when={isMobileViewport()}
            fallback={
              <div class="h-full min-h-0 min-w-0">
                <Resizable class="h-full w-full min-h-0 min-w-0" initialSizes={[0.72, 0.28]} keyboardDelta={0.03}>
                  <ResizablePanel class="min-h-0 min-w-0 overflow-hidden" minSize={0.35} initialSize={0.72}>
                    {mainColumn()}
                  </ResizablePanel>
                  <ResizableHandle aria-label="Resize git panel" class="z-[2] w-px basis-px px-0 bg-[var(--line)] hover:bg-[var(--line-strong)]" />
                  <ResizablePanel class="min-h-0 min-w-0 overflow-hidden" minSize={0.18} maxSize={0.55} initialSize={0.28}>
                    <GitSidePanel git={git} variant="desktop" />
                  </ResizablePanel>
                </Resizable>
              </div>
            }
          >
            <div class="h-full min-h-0 min-w-0 max-[900px]:h-[var(--dvh,100dvh)] max-[900px]:max-h-[var(--dvh,100dvh)]">
              {mainLayout()}
            </div>
          </Show>
        </Show>
      </div>

      <CommandPalette command={ui.commandPalette} />
      <ServerManagerDialog servers={ui.servers} />
      <ImagePreviewDialog image={ui.imagePreview} onClose={ui.onCloseImagePreview} />
    </div>
  )
}

function PairOverlay(props: { pair: PairPanelController }) {
  const pair = props.pair

  return (
    <Show when={pair.required()}>
      <div class="fixed inset-0 z-[100] grid items-start justify-items-center bg-black/60 px-4 pt-[min(14vh,7rem)] pb-4">
        <form class={cx(PANEL_BASE, "w-[min(100%,28rem)] p-5")} onSubmit={pair.onSubmit}>
          <h1 class="m-0 text-[1.1rem] font-semibold text-[var(--text)]">Pair device</h1>
          <p class="my-3 text-[var(--muted)] leading-relaxed">Enter the code printed by crabcode serve.</p>
          <div class="grid grid-cols-[1fr_auto] gap-2 max-[560px]:grid-cols-1">
            <input
              class={cx(INPUT_BASE, "h-10 font-mono")}
              value={pair.code()}
              onInput={(event) => pair.setCode(event.currentTarget.value)}
              autocomplete="one-time-code"
              inputmode="numeric"
              placeholder="482-119"
            />
            <button class="h-10 rounded-lg bg-[#e5e2dc] px-4 font-bold text-[#171717]" type="submit">
              Connect
            </button>
          </div>
          <div class="mt-3 min-h-4 text-[0.82rem] text-[var(--red)]">{pair.error()}</div>
        </form>
      </div>
    </Show>
  )
}

function ProjectPathInlineForm(props: {
  form: ProjectPathFormController
  class?: string
  showError?: boolean
}) {
  return (
    <>
      <form class={cx("grid grid-cols-[minmax(0,1fr)_auto] gap-2", props.class)} onSubmit={props.form.onSubmit}>
        <input
          class={cx(INPUT_BASE, "h-10 font-mono text-[0.78rem]")}
          ref={props.form.setInputRef}
          value={props.form.value()}
          onInput={(event) => props.form.setValue(event.currentTarget.value)}
          placeholder="/Users/carlo/Desktop/Projects/app"
        />
        <button
          class="h-10 rounded-lg bg-[#e5e2dc] px-3 text-[0.82rem] font-bold text-[#171717]"
          type="submit"
          disabled={!props.form.value().trim()}
        >
          Open
        </button>
      </form>
      <Show when={props.showError && props.form.error()}>
        <div class="mt-2 text-[0.76rem] leading-snug text-[var(--red)]">{props.form.error()}</div>
      </Show>
    </>
  )
}

function RemoteSidebar(props: { sidebar: SidebarController }) {
  const sidebar = props.sidebar

  return (
    <>
      <aside
        class={cx(
          "flex h-dvh min-w-0 flex-col overflow-hidden border-r border-[var(--line)] bg-[var(--panel)] max-[900px]:fixed max-[900px]:inset-y-0 max-[900px]:left-0 max-[900px]:z-[80] max-[900px]:w-[min(25rem,88vw)] max-[900px]:transition-transform max-[900px]:duration-150",
          sidebar.open() ? "max-[900px]:translate-x-0" : "max-[900px]:-translate-x-[101%]"
        )}
      >
        <button
          class="group mx-6 mt-4 mb-5 flex items-center gap-2 rounded-lg bg-[#1d1d1d] px-3 py-2 text-[var(--muted)] transition hover:bg-[#282828] hover:text-[var(--text)]"
          type="button"
          onClick={sidebar.onOpenCommandPalette}
        >
          <IconSearch class="h-[1.1rem] w-[1.1rem] text-[var(--faint)] transition group-hover:text-[var(--text)]" />
          <span class="min-w-0 flex-1 text-left text-[0.95rem] text-[var(--muted)] transition group-hover:text-[var(--text)]">Search</span>
          <span class="inline-flex items-center gap-px rounded-md border border-[var(--line-strong)] bg-[#202020] px-1.5 py-1 font-mono text-[0.72rem] text-[var(--muted)]">
            <span>⌘</span>
            <span class="w-1" aria-hidden="true" />
            <span>K</span>
          </span>
        </button>

        <div class="flex items-center justify-between px-6 pb-2 text-[0.72rem] font-bold uppercase tracking-[0.08em] text-[var(--faint)]">
          <span>Projects</span>
          <div class="flex items-center gap-0.5">
            <button
              class={cx(ICON_BUTTON, "h-7 w-7")}
              type="button"
              title={sidebar.allProjectsExpanded() ? "Collapse all projects" : "Expand all projects"}
              onClick={sidebar.onToggleAllProjects}
            >
              <IconF7ChevronDownSquare
                class="h-4 w-4 transition-transform duration-[180ms]"
                aria-hidden="true"
                style={{
                  transform: sidebar.allProjectsExpanded() ? "rotate(0deg)" : "rotate(-90deg)",
                }}
              />
            </button>
            <Popover
              open={sidebar.newProjectOpen()}
              onOpenChange={sidebar.onNewProjectOpenChange}
              placement="bottom-end"
              gutter={8}
            >
              <PopoverTrigger as="button" class={cx(ICON_BUTTON, "h-7 w-7")} type="button" title="Open folder">
                <IconPlus class="h-4 w-4" />
              </PopoverTrigger>
              <PopoverContent class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] w-[min(24rem,calc(100vw-1.4rem))] p-3")}>
                <ProjectPathInlineForm form={sidebar.projectPathForm} showError />
              </PopoverContent>
            </Popover>
          </div>
        </div>

        <ProjectList
          projects={sidebar.projects}
          openProjects={sidebar.openProjects}
          activeProjectPath={sidebar.activeProjectPath}
          token={sidebar.token}
          currentSessionId={sidebar.currentSessionId}
          onToggleProject={sidebar.onToggleProject}
          onNewSession={sidebar.onNewSession}
          onSwitchSession={sidebar.onSwitchSession}
          onArchiveSession={sidebar.onArchiveSession}
          onArchiveProject={sidebar.onArchiveProject}
        />
      </aside>
    </>
  )
}

function MainHeader(props: { header: HeaderController }) {
  const header = props.header

  return (
    <header class="flex h-[4.8rem] flex-none items-center justify-between gap-4 border-b border-[var(--line)] bg-[#181818] px-8 max-[900px]:px-4">
      <button
        class="hidden h-[2.2rem] w-[2.2rem] place-items-center rounded-lg border border-[var(--line)] text-[var(--muted)] max-[900px]:inline-grid"
        type="button"
        onClick={() => header.setSidebarOpen(true)}
        aria-label="Open projects"
      >
        <IconSidebar class="h-[1.1rem] w-[1.1rem]" />
      </button>
      <ProjectPicker picker={header.projectPicker} />
      <div class="ml-auto flex items-center gap-2">
        <Show when={!header.isEmptyChat()}>
          <button
            class="inline-flex h-[2.2rem] max-w-[9.5rem] min-w-0 items-center gap-2 rounded-lg border border-[var(--line-strong)] bg-[#222222] px-3 text-[0.86rem] font-semibold text-[#d7d5d0] transition hover:border-[rgba(255,255,255,0.18)] hover:bg-[#2b2b2b] hover:text-[var(--text)] max-[560px]:aspect-square max-[560px]:w-[2.2rem] max-[560px]:max-w-none max-[560px]:justify-center max-[560px]:p-0"
            type="button"
            title="New chat"
            onClick={() => header.onNewSession()}
          >
            <IconPlus class="h-4 w-4 shrink-0" />
            <span class="min-w-0 truncate whitespace-nowrap max-[560px]:hidden">New chat</span>
          </button>
        </Show>
        <GitPanelTrigger git={header.gitViewer} />
        <ServerPopover servers={header.servers} />
      </div>
    </header>
  )
}

function GitPanelTrigger(props: { git: GitViewerController }) {
  const git = props.git
  const summary = createMemo(() => git.summary())
  const status = createMemo(() => git.status())
  const branchLabel = createMemo(() => status()?.branch || summary()?.branch || "git")
  const changedCount = createMemo(() => status()?.changed_files ?? null)

  return (
    <Show when={summary()?.is_repo}>
      <button
        class={cx(
          "inline-flex h-[2.2rem] min-w-0 items-center gap-2 rounded-lg border px-2.5 text-[#d7d5d0] transition max-[560px]:aspect-square max-[560px]:w-[2.2rem] max-[560px]:justify-center max-[560px]:p-0",
          git.open()
            ? "border-[rgba(108,142,216,0.45)] bg-[#252a33] text-[var(--text)]"
            : "border-[var(--line-strong)] bg-[#1f1f1f] hover:border-[rgba(255,255,255,0.18)] hover:bg-[#252525]"
        )}
        type="button"
        aria-label="Toggle git changes"
        aria-pressed={git.open()}
        title="Git changes"
        onClick={() => git.onOpenChange(!git.open())}
      >
        <IconGitDiff class="h-[1.05rem] w-[1.05rem] text-[var(--muted)]" />
        <span class="max-w-[9rem] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.76rem] font-semibold text-[var(--muted)] max-[560px]:hidden">
          {branchLabel()}
        </span>
        <Show when={changedCount() !== null && changedCount()! > 0}>
          <span class="rounded-full bg-[var(--brand-primary)]/20 px-1.5 py-0.5 font-mono text-[0.66rem] font-bold text-[var(--brand-primary)] max-[560px]:hidden">
            {changedCount()}
          </span>
        </Show>
      </button>
    </Show>
  )
}

function GitSidePanel(props: { git: GitViewerController; variant: "desktop" | "mobile" }) {
  const git = props.git
  const status = createMemo(() => git.status())
  const branchLabel = createMemo(() => status()?.branch || git.summary()?.branch || "git")
  const [wrapLines, setWrapLines] = createSignal(false)

  return (
    <aside
      class={cx(
        "flex h-dvh min-h-0 min-w-0 flex-col overflow-hidden border-[var(--line)] bg-[var(--panel)]",
        props.variant === "desktop"
          ? "h-full w-full border-l"
          : "fixed inset-y-0 right-0 z-[80] w-[min(25rem,92vw)] border-l shadow-[0_0_3rem_rgba(0,0,0,0.45)] min-[901px]:hidden"
      )}
    >
      <div class="flex flex-none items-center justify-between gap-2 border-b border-[var(--line)] px-3 py-2.5">
        <div class="min-w-0">
          <div class="text-[0.84rem] font-bold text-[var(--text)]">Changes</div>
          <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.68rem] text-[var(--faint)]">
            {branchLabel()}
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-1">
          <button
            class={cx(
              "inline-flex h-7 items-center justify-center rounded-md border px-2.5 text-[0.72rem] font-semibold transition disabled:opacity-55",
              wrapLines()
                ? "border-[var(--line-strong)] bg-white/[0.08] text-[var(--text)]"
                : "border-[var(--line-strong)] text-[var(--muted)] hover:bg-white/[0.045] hover:text-[var(--text)]"
            )}
            type="button"
            aria-pressed={wrapLines()}
            title={wrapLines() ? "Disable line wrap" : "Wrap long lines"}
            onClick={() => setWrapLines((value) => !value)}
          >
            Wrap
          </button>
          <button
            class="inline-flex h-7 items-center justify-center rounded-md border border-[var(--line-strong)] px-2.5 text-[0.72rem] font-semibold text-[var(--muted)] transition hover:bg-white/[0.045] hover:text-[var(--text)] disabled:opacity-55"
            type="button"
            disabled={git.loading()}
            onClick={() => git.onRefresh()}
          >
            {git.loading() ? "…" : "Refresh"}
          </button>
          <button
            class={cx(ICON_BUTTON, "h-7 w-7")}
            type="button"
            aria-label="Close git panel"
            onClick={() => git.onOpenChange(false)}
          >
            <IconX class="h-4 w-4" />
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-3 py-2.5">
        <Show when={git.error()}>
          {(error) => (
            <div class="mb-2 rounded-md border border-[rgba(200,108,116,0.28)] bg-[rgba(200,108,116,0.06)] px-2.5 py-1.5 text-[0.76rem] leading-snug text-[var(--red)]">
              {error()}
            </div>
          )}
        </Show>
        <Show when={status()} fallback={<GitLoadingState loading={git.loading} />}>
          {(current) => (
            <GitStatusView git={git} status={current} loading={git.loading} wrapLines={wrapLines} />
          )}
        </Show>
      </div>
    </aside>
  )
}

function GitLoadingState(props: { loading: () => boolean }) {
  return (
    <div class="py-6 text-center text-[0.8rem] text-[var(--muted)]">
      {props.loading() ? "Loading changes…" : "Loading…"}
    </div>
  )
}

function GitStatusView(props: {
  git: GitViewerController
  status: () => NonNullable<ReturnType<GitViewerController["status"]>>
  loading: () => boolean
  wrapLines: () => boolean
}) {
  const status = props.status
  const selectedDiff = createMemo(() => {
    const path = props.git.selectedPath()
    if (!path) return null
    return status().diff_files.find((file) => file.path === path) ?? null
  })

  return (
    <div class={cx("grid min-w-0 gap-2.5", props.loading() && "opacity-70")}>
      <GitStatsRow
        git={props.git}
        hasDiffs={status().diff_files.length > 0}
        additions={status().additions}
        deletions={status().deletions}
      />

      <Show
        when={status().changed_files > 0}
        fallback={<div class="py-4 text-center text-[0.8rem] text-[var(--muted)]">Working tree is clean.</div>}
      >
        <section class="min-h-0 min-w-0">
          <div class="mb-1 flex items-center gap-1.5 px-0.5 text-[0.66rem] font-bold uppercase tracking-[0.08em] text-[var(--faint)]">
            <span>Files</span>
            <span class="rounded-full border border-[var(--line)] bg-white/[0.04] px-1.5 py-px font-mono text-[0.58rem] leading-none tracking-normal text-[var(--muted)]">
              {status().changed_files}
            </span>
          </div>
          <div class="max-h-[min(14rem,28vh)] min-w-0 overflow-y-auto rounded-md border border-[var(--line)] bg-black/10 p-0.5">
            <For each={status().files}>
              {(file) => (
                <GitFileRow
                  file={file}
                  selected={props.git.selectedPath() === file.path}
                  onSelect={() => {
                    props.git.setSelectedPath(file.path)
                    props.git.setViewMode("file")
                  }}
                />
              )}
            </For>
          </div>
        </section>

        <section class="min-h-0 min-w-0 overflow-hidden">
          <Show
            when={props.git.viewMode() === "all"}
            fallback={
              <Show
                when={selectedDiff()}
                fallback={
                  <div class="rounded-md border border-[var(--line)] bg-white/[0.02] px-2.5 py-3 text-[0.76rem] text-[var(--faint)]">
                    No diff preview for this file.
                  </div>
                }
              >
                {(file) => <GitDiffFile file={file()} compact wrap={props.wrapLines()} />}
              </Show>
            }
          >
            <Show
              when={status().diff_files.length > 0}
              fallback={
                <div class="rounded-md border border-[var(--line)] bg-white/[0.02] px-2.5 py-3 text-[0.76rem] text-[var(--faint)]">
                  No textual diffs to preview.
                </div>
              }
            >
              <div class="grid min-w-0 gap-2">
                <For each={status().diff_files}>
                  {(file) => <GitDiffFile file={file} compact wrap={props.wrapLines()} />}
                </For>
              </div>
            </Show>
          </Show>
        </section>

        <Show when={status().truncated}>
          <div class="rounded-md border border-[#c9a24a]/22 bg-[#c9a24a]/7 px-2.5 py-1.5 text-[0.72rem] leading-snug text-[#d4bc82]">
            Large diff truncated. Refresh after narrowing changes for more.
          </div>
        </Show>
      </Show>
    </div>
  )
}

function GitStatsRow(props: {
  git: GitViewerController
  hasDiffs: boolean
  additions: number
  deletions: number
}) {
  const mode = () => props.git.viewMode()
  const tabClass = (active: boolean) =>
    cx(
      "rounded-[5px] px-2 py-0.5 font-semibold transition",
      active ? "bg-white/[0.1] text-[var(--text)]" : "text-[var(--faint)] hover:bg-white/[0.04] hover:text-[var(--muted)]"
    )

  return (
    <div class="flex flex-wrap items-center gap-1.5 text-[0.72rem]">
      <Show when={props.hasDiffs}>
        <span class="inline-flex items-center gap-0.5 rounded-md border border-[var(--line)] bg-black/10 p-0.5 font-mono">
          <button class={tabClass(mode() === "all")} type="button" onClick={() => props.git.setViewMode("all")}>
            All
          </button>
          <button class={tabClass(mode() === "file")} type="button" onClick={() => props.git.setViewMode("file")}>
            File
          </button>
        </span>
        <span class="inline-flex items-center gap-1 rounded-md border border-[rgba(72,158,108,0.28)] bg-[rgba(46,120,82,0.12)] px-2 py-0.5 font-mono font-semibold text-[#6ecf9a]">
          <span class="text-[0.62rem] font-bold uppercase tracking-[0.06em] text-[#5aab7d]">add</span>
          +{props.additions}
        </span>
        <span class="inline-flex items-center gap-1 rounded-md border border-[rgba(196,98,108,0.28)] bg-[rgba(120,48,58,0.14)] px-2 py-0.5 font-mono font-semibold text-[#f08a96]">
          <span class="text-[0.62rem] font-bold uppercase tracking-[0.06em] text-[#c4727c]">del</span>
          −{props.deletions}
        </span>
      </Show>
    </div>
  )
}

function GitFileRow(props: {
  file: NonNullable<ReturnType<GitViewerController["status"]>>["files"][number]
  selected: boolean
  onSelect: () => void
}) {
  return (
    <button
      class={cx(
        "grid w-full min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-1.5 rounded-[5px] px-1.5 py-1 text-left transition",
        props.selected ? "bg-white/[0.08]" : "hover:bg-white/[0.04]"
      )}
      type="button"
      onClick={props.onSelect}
    >
      <span class={cx("rounded px-1 py-px font-mono text-[0.6rem] font-bold uppercase", gitStatusClass(props.file.status))}>
        {gitStatusLabel(props.file.status)}
      </span>
      <span
        class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.7rem] text-[#d4d2cd]"
        title={props.file.old_path ? `${props.file.old_path} → ${props.file.path}` : props.file.path}
      >
        <Show when={props.file.old_path} fallback={props.file.path}>
          {(oldPath) => (
            <>
              {oldPath()} → {props.file.path}
            </>
          )}
        </Show>
      </span>
      <span class="inline-flex items-center gap-1 whitespace-nowrap font-mono text-[0.65rem] font-semibold">
        <Show
          when={props.file.binary}
          fallback={
            <>
              <span class="text-[#6ecf9a]">+{props.file.additions}</span>
              <span class="text-[#f08a96]">−{props.file.deletions}</span>
            </>
          }
        >
          <span class="text-[var(--faint)]">bin</span>
        </Show>
      </span>
    </button>
  )
}

function gitStatusLabel(status: string) {
  if (status === "added") return "A"
  if (status === "deleted") return "D"
  if (status === "renamed") return "R"
  if (status === "untracked") return "U"
  return "M"
}

function gitStatusClass(status: string) {
  if (status === "added" || status === "untracked") return "bg-[#0c2613] text-[#8fd8aa]"
  if (status === "deleted") return "bg-[#2b1012] text-[#e09299]"
  if (status === "renamed") return "bg-[#1f2536] text-[#8fb7ff]"
  return "bg-white/[0.055] text-[var(--muted)]"
}

function ProjectPicker(props: { picker: ProjectPickerController }) {
  const picker = props.picker

  const showAddProjectForm = () => {
    picker.setAddOpen((open) => !open)
    picker.form.setError("")
    picker.form.setValue("")
    picker.form.focusInput()
  }

  return (
    <Popover
      open={picker.open()}
      onOpenChange={picker.onOpenChange}
      placement="bottom-start"
      gutter={8}
    >
      <PopoverTrigger
        as="button"
        class="grid min-w-0 max-w-[min(36rem,52vw)] flex-[0_1_auto] grid-cols-[auto_minmax(0,auto)_auto] items-center justify-start gap-2 rounded-lg px-2 py-1.5 text-left transition hover:bg-white/[0.035]"
        type="button"
      >
        <Show
          when={picker.projectName()}
          fallback={<span class="h-[1.8rem] w-[1.8rem] rounded-md bg-white/[0.04]" aria-hidden="true" />}
        >
          <ProjectFavicon
            cwd={picker.projectPath()}
            label={picker.projectName()}
            token={picker.token()}
            class="h-[1.8rem] w-[1.8rem]"
          />
        </Show>
        <span class="flex min-w-0 flex-col gap-0.5">
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[1.12rem] font-bold text-[var(--text)]">
            <Show
              when={picker.projectName()}
              fallback={
                <span class="flex h-[1lh] w-24 items-center" aria-hidden="true">
                  <span class="block h-[1em] w-full rounded-sm bg-white/[0.04]" />
                </span>
              }
            >
              {picker.projectName()}
            </Show>
          </span>
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.72rem] text-[var(--faint)] max-[560px]:hidden">
            {picker.projectPath() || "\u00a0"}
          </span>
        </span>
        <IconCaretDown class="h-3 w-3 text-[var(--faint)]" />
      </PopoverTrigger>
      <PopoverContent
        class={cx(
          PANEL_BASE,
          POPOVER_ANIMATION,
          "z-[90] flex max-h-[min(26rem,70vh)] w-[min(24rem,calc(100vw-1.4rem))] flex-col overflow-hidden"
        )}
      >
        <div class="flex items-center justify-between gap-3 border-b border-[var(--line)] py-3 pr-2 pl-3 text-[0.72rem] font-bold uppercase tracking-[0.07em] text-[var(--muted)]">
          <span>Open project</span>
          <button
            class={cx(ICON_BUTTON, "h-[1.65rem] w-[1.65rem]")}
            type="button"
            title="Add project"
            onClick={showAddProjectForm}
          >
            <IconPlus class="h-[0.95rem] w-[0.95rem]" />
          </button>
        </div>
        <Show when={picker.addOpen()}>
          <ProjectPathInlineForm form={picker.form} class="border-b border-[var(--line)] p-3" />
        </Show>
        <div class="min-h-0 flex-1 overflow-y-auto p-2">
          <For each={picker.projects()}>
            {(project) => {
              const latest = () => mostRecentSession(project.sessions)
              const hasResume = () => project.sessions.length > 0
              return (
                <div
                  class={cx(
                    "group relative min-w-0 rounded-lg text-[var(--text)] transition hover:bg-white/[0.055]",
                    project.path === picker.projectPath()
                      && "bg-white/[0.055] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.035)]"
                  )}
                >
                  <button
                    class={cx(
                      "grid w-full min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-lg px-2 py-2 text-left",
                      hasResume() && "pr-[2.65rem]"
                    )}
                    type="button"
                    onClick={() => picker.onSelectWorkspace(project.path)}
                  >
                    <ProjectFavicon
                      cwd={project.path}
                      label={project.name}
                      token={picker.token()}
                      class="h-[1.35rem] w-[1.35rem]"
                    />
                    <span class="flex min-w-0 flex-col gap-0.5">
                      <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">
                        {project.name}
                      </span>
                      <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.68rem] text-[var(--faint)]">
                        {project.path}
                      </span>
                    </span>
                  </button>
                  <Show when={hasResume()}>
                    <button
                      class="absolute top-1/2 right-1.5 z-10 grid h-[1.85rem] w-[1.85rem] -translate-y-1/2 place-items-center rounded-md text-[var(--faint)] opacity-75 transition hover:bg-[#2d2d2d] hover:text-[var(--text)] hover:opacity-100 focus-visible:bg-[#2d2d2d] focus-visible:text-[var(--text)] focus-visible:opacity-100 group-hover:opacity-100"
                      type="button"
                      title={
                        latest()
                          ? `Resume “${latest()!.title}” (${relativeTime(latest()!.updated_at)})`
                          : `Resume latest chat in ${project.name}`
                      }
                      aria-label={`Resume latest chat in ${project.name}`}
                      onClick={(event) => {
                        event.stopPropagation()
                        void picker.onResumeProject(project.path)
                      }}
                    >
                      <IconClockCounterClockwise class="h-[1.05rem] w-[1.05rem]" />
                    </button>
                  </Show>
                </div>
              )
            }}
          </For>
        </div>
        <Show when={picker.form.error()}>
          <div class="border-t border-[var(--line)] px-3 py-2 text-[0.76rem] leading-snug text-[var(--red)]">
            {picker.form.error()}
          </div>
        </Show>
      </PopoverContent>
    </Popover>
  )
}

function ServerPopover(props: { servers: ServerPanelController }) {
  const servers = props.servers

  return (
    <Popover open={servers.popoverOpen()} onOpenChange={servers.onPopoverOpenChange} placement="bottom-end" gutter={8}>
      <PopoverTrigger
        as="button"
        class="relative inline-flex h-[2.2rem] w-[2.2rem] items-center justify-center rounded-lg border border-[var(--line-strong)] bg-[#1f1f1f] p-0 text-[#d7d5d0] transition hover:bg-[#252525]"
        type="button"
        aria-label="Open servers"
        title="Open servers"
      >
        <span class="absolute top-[0.42rem] right-[0.42rem] flex h-[0.43rem] w-[0.43rem]">
          <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-[#53b842] opacity-60" />
          <span class="relative inline-flex h-full w-full rounded-full bg-[#53b842]" />
        </span>
        <IconServers class="h-[1.08rem] w-[1.08rem] text-[var(--muted)]" />
      </PopoverTrigger>
      <PopoverContent
        class={cx(
          PANEL_BASE,
          POPOVER_ANIMATION,
          "z-[90] w-[min(27rem,calc(100vw-1.4rem))] overflow-hidden p-3"
        )}
      >
        <div class="flex items-center gap-4 border-b border-[var(--line)] px-0.5 pb-3">
          <button
            class={cx(
              "inline-flex items-center gap-1.5 py-1 text-[0.9rem] text-[var(--muted)]",
              servers.tab() === "servers" && "border-b-2 border-[var(--text)] text-[var(--text)]"
            )}
            type="button"
            onClick={() => servers.onSelectTab("servers")}
          >
            Servers <CountBadge count={servers.servers().length} />
          </button>
          <button
            class={cx(
              "inline-flex items-center gap-1.5 py-1 text-[0.9rem] text-[var(--muted)]",
              servers.tab() === "skills" && "border-b-2 border-[var(--text)] text-[var(--text)]"
            )}
            type="button"
            onClick={() => servers.onSelectTab("skills")}
          >
            Skills <CountBadge count={servers.skills().length} />
          </button>
          <button
            class={cx(
              "inline-flex items-center gap-1.5 py-1 text-[0.9rem] text-[var(--muted)]",
              servers.tab() === "mcp" && "border-b-2 border-[var(--text)] text-[var(--text)]"
            )}
            type="button"
            onClick={() => servers.onSelectTab("mcp")}
          >
            MCP <CountBadge count={servers.mcpServers().length} />
          </button>
          <button
            class={cx("py-1 text-[0.9rem] text-[var(--muted)]", servers.tab() === "lsp" && "text-[var(--text)]")}
            type="button"
            onClick={() => servers.onSelectTab("lsp")}
            disabled
          >
            LSP
          </button>
          <button
            class={cx("py-1 text-[0.9rem] text-[var(--muted)]", servers.tab() === "plugins" && "text-[var(--text)]")}
            type="button"
            onClick={() => servers.onSelectTab("plugins")}
            disabled
          >
            Plugins
          </button>
        </div>
        <Show
          when={servers.tab() === "skills"}
          fallback={
            <Show
              when={servers.tab() === "mcp"}
              fallback={
                <>
                  <div class="grid gap-1 py-3">
                    <For each={servers.servers().slice(0, 3)}>
                      {(server) => (
                        <button
                          class="grid min-h-10 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-2 rounded-lg px-2 text-left text-[var(--muted)] hover:bg-white/[0.04] hover:text-[var(--text)]"
                          type="button"
                          onClick={() => servers.onOpenServer(server)}
                        >
                          <span class="h-2 w-2 rounded-full bg-[#53b842]" />
                          <span>{server.name}</span>
                          <span class="text-[0.78rem] text-[var(--faint)]">v{servers.status()?.version}</span>
                          <Show when={isActiveServer(server.address, servers.activeServerUrl())}>
                            <IconCheck class="h-4 w-4 text-[var(--muted)]" />
                          </Show>
                        </button>
                      )}
                    </For>
                  </div>
                  <button
                    class="inline-flex h-9 items-center justify-center rounded-lg border border-[var(--line-strong)] px-3 text-[0.84rem] font-medium text-[var(--text)] hover:bg-white/[0.045]"
                    type="button"
                    onClick={servers.onOpenManager}
                  >
                    Manage servers
                  </button>
                </>
              }
            >
              <div class="grid max-h-76 gap-0.5 overflow-auto py-3 pb-1">
                <Show
                  when={servers.mcpServers().length > 0}
                  fallback={
                    <div class="overflow-hidden text-ellipsis whitespace-nowrap px-2 text-[0.78rem] text-[var(--muted)]">
                      No MCP servers configured.
                    </div>
                  }
                >
                  <For each={servers.mcpServers()}>
                    {(server) => (
                      <button
                        class="grid min-h-10 w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-3 rounded-[9px] px-2 py-2 text-left text-[var(--text)] hover:bg-white/[0.045] disabled:opacity-60"
                        type="button"
                        disabled={servers.mcpToggling() === server.name}
                        onClick={() => servers.onToggleMcpServer(server.name)}
                      >
                        <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.9rem] font-medium">
                          {server.name}
                        </span>
                        <span
                          class={cx(
                            "shrink-0 text-[0.78rem] font-medium tabular-nums",
                            server.enabled ? "text-[#53b842]" : "text-[var(--faint)]"
                          )}
                        >
                          {server.enabled ? "Enabled" : "Disabled"}
                        </span>
                      </button>
                    )}
                  </For>
                </Show>
              </div>
            </Show>
          }
        >
          <div class="grid max-h-76 gap-1 overflow-auto py-3 pb-1">
            <Show when={servers.skills().length > 0} fallback={<div class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">No skills loaded.</div>}>
              <For each={servers.skills()}>
                {(skill) => (
                  <div class="grid grid-cols-[1.45rem_minmax(0,1fr)] items-start gap-2 rounded-[9px] px-2 py-2 hover:bg-white/[0.045]">
                    <IconBrainGlyph class="mt-0.5 h-5 w-5 text-[#d9a6ff]" />
                    <span class="grid min-w-0 gap-0.5">
                      <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.9rem] font-semibold text-[var(--text)]">
                        {skill.name}
                      </span>
                      <small class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">
                        {skill.description || skill.location}
                      </small>
                    </span>
                  </div>
                )}
              </For>
            </Show>
          </div>
        </Show>
      </PopoverContent>
    </Popover>
  )
}

function ThreadViewport(props: { thread: ThreadController }) {
  const thread = props.thread

  return (
    <div class="relative min-h-0 flex-1">
      <div
        ref={thread.setScrollRef}
        class={cx(
          "remote-thread-scroll h-full min-h-0 overflow-y-auto overflow-x-hidden px-4 pt-5 overscroll-contain",
          thread.isEmptyChat()
            ? "overflow-hidden pb-[clamp(8rem,22vh,12rem)] max-[900px]:pb-4"
            : thread.isSubagentView()
              ? "pb-8 max-[900px]:pb-4 max-[900px]:pt-2"
              : "pb-52 max-[900px]:pb-4 max-[900px]:pt-2"
        )}
      >
        <div
          ref={thread.setContentRef}
          class={cx(
            "mx-auto w-[min(100%,64rem)] min-w-0 max-w-full",
            (thread.isEmptyChat() || thread.shellLoading()) && "grid h-full"
          )}
        >
          <Show when={!thread.shellLoading()} fallback={<div class="min-h-0" aria-busy="true" aria-label="Loading" />}>
            <Show
              when={thread.visibleMessages().length > 0}
              fallback={
                <EmptyThread
                  projectName={thread.projectName()}
                  mascotFrame={thread.mascotFrame()}
                  recentSessions={thread.recentSessions}
                  onSwitchSession={thread.onSwitchSession}
                />
              }
            >
              <Index each={thread.threadItems()}>
                {(item) => (
                  <ThreadItemView
                    item={item}
                    status={thread.status}
                    streaming={thread.streaming}
                    token={thread.token}
                    onPreviewImage={thread.onPreviewImage}
                    onOpenSubagentSession={thread.tabs.onOpenSubagentSession}
                  />
                )}
              </Index>
            </Show>
          </Show>
        </div>
      </div>
      <FadedEdgeEffect direction="top" hidden={thread.isAtTop()} size="3rem" color="#171717" />
      <FadedEdgeEffect direction="bottom" hidden={thread.isAtBottom()} size="7rem" color="#171717" />
    </div>
  )
}

function CommandPalette(props: { command: CommandPaletteController }) {
  const command = props.command

  return (
    <Show when={command.rendered()}>
      <div
        class={cx(
          "fixed inset-0 z-[100] grid place-items-start justify-items-center bg-black/60 px-4 pt-[min(14vh,7rem)] pb-4",
          command.closing() ? "animate-fadeOut" : "animate-fadeIn"
        )}
        onMouseDown={(event) => event.currentTarget === event.target && command.onClose()}
      >
        <div
          class={cx(
            PANEL_BASE,
            "grid max-h-[min(34rem,calc(100dvh-min(14vh,7rem)-1rem))] w-[min(100%,43rem)] grid-rows-[auto_minmax(0,1fr)] overflow-hidden origin-top will-change-transform",
            command.closing() ? "animate-flyUpAndScaleExit" : "animate-flyUpAndScale"
          )}
        >
          <Command class="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)]" shouldFilter={false} loop>
            <div class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 border-b border-[var(--line)] px-3" cmdk-input-wrapper="">
              <IconSearch class="h-4 w-4 text-[var(--faint)]" />
              <CommandInput
                ref={command.setInputRef}
                class="h-[2.65rem] min-w-0 border-0 bg-transparent text-[var(--text)] outline-none"
                placeholder="Search projects and sessions"
                value={command.query()}
                onValueChange={command.setQuery}
                onKeyDown={(event) => {
                  if (event.key === "Escape") command.onClose()
                }}
              />
            </div>
            <CommandList class="min-h-0 overflow-y-auto overscroll-contain p-2">
              <CommandEmpty class="px-2 py-4 text-[0.84rem] text-[var(--faint)]">No projects or sessions found.</CommandEmpty>
              <Show when={!command.isEmptyChat()}>
                <CommandGroup heading="Actions" forceMount>
                  <CommandItem
                    class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                    value="new-chat"
                    onSelect={() => command.onNewSession()}
                    forceMount
                  >
                    <div class="min-w-0 flex-1">
                      <div class="min-w-0 truncate text-[0.86rem] font-semibold">New chat</div>
                      <div class="block min-w-0 truncate text-[0.72rem] text-[var(--faint)]">Start a blank session in this workspace</div>
                    </div>
                    <IconPlus class="h-4 w-4" />
                  </CommandItem>
                </CommandGroup>
              </Show>
              <CommandGroup heading="Projects" forceMount>
                <For each={command.projectResults()}>
                  {(project) => (
                    <CommandItem
                      class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                      value={`project-${project.path}`}
                      keywords={[project.name, project.path]}
                      onSelect={() => command.onNewSession(project.path)}
                      forceMount
                    >
                      <div>
                        <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">{project.name}</div>
                        <div class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">{project.path}</div>
                      </div>
                      <IconFolder class="h-4 w-4" />
                    </CommandItem>
                  )}
                </For>
              </CommandGroup>
              <CommandGroup heading="Sessions" forceMount>
                <For each={command.sessionResults()}>
                  {(session) => (
                    <CommandItem
                      class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                      value={session.id}
                      keywords={[session.title, session.workspace]}
                      onSelect={() => command.onSwitchSession(session.id)}
                      forceMount
                    >
                      <div>
                        <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">{session.title || "Untitled chat"}</div>
                        <div class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">{session.workspace}</div>
                      </div>
                      <span class="whitespace-nowrap text-[0.72rem] text-[var(--faint)]">{relativeTime(session.updated_at)}</span>
                    </CommandItem>
                  )}
                </For>
              </CommandGroup>
            </CommandList>
          </Command>
        </div>
      </div>
    </Show>
  )
}

function ServerManagerDialog(props: { servers: ServerPanelController }) {
  const servers = props.servers

  return (
    <Show when={servers.manageOpen()}>
      <div
        class="fixed inset-0 z-[120] grid place-items-center bg-black/55 p-4 animate-fadeIn"
        onMouseDown={(event) => event.currentTarget === event.target && servers.setManageOpen(false)}
      >
        <section class="flex max-h-[min(42rem,calc(100dvh-2rem))] w-[min(100%,48rem)] flex-col overflow-hidden rounded-[14px] border border-[var(--line-strong)] bg-[#1a1a1a] shadow-[0_1.2rem_4rem_rgba(0,0,0,0.42)] animate-flyUpAndScale">
          <div class="flex min-h-[4.2rem] flex-none items-center justify-between gap-4 px-6">
            <Show
              when={servers.addOpen()}
              fallback={<h2 class="m-0 text-[1.2rem] font-semibold text-[var(--text)]">Servers</h2>}
            >
              <button class="inline-flex items-center gap-3 text-[var(--muted)] hover:text-[var(--text)]" type="button" onClick={() => servers.setAddOpen(false)}>
                <IconArrowLeft class="h-[1.15rem] w-[1.15rem]" />
                <span>Add server</span>
              </button>
            </Show>
            <button
              class="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] hover:text-[var(--text)]"
              type="button"
              onClick={() => servers.setManageOpen(false)}
            >
              <IconX class="h-5 w-5" />
            </button>
          </div>

          <Show
            when={servers.addOpen()}
            fallback={
              <>
                <div class="mx-6 mb-4 grid min-w-0 flex-none grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-[9px] bg-[#181818] px-3">
                  <IconSearch class="h-4 w-4 text-[var(--muted)]" />
                  <input
                    class="h-11 min-w-0 border-0 bg-transparent text-[var(--text)] outline-none"
                    value={servers.search()}
                    onInput={(event) => servers.setSearch(event.currentTarget.value)}
                    placeholder="Search servers"
                  />
                </div>
                <div class="grid min-h-0 flex-1 gap-2 overflow-y-auto px-6 pb-4">
                  <For each={servers.filteredServers()}>
                    {(server) => (
                      <button
                        class="grid min-h-[4.6rem] min-w-0 grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-3 rounded-[9px] bg-[#1f1f1f] px-4 text-left text-[var(--text)] hover:bg-[#242424]"
                        type="button"
                        onClick={() => servers.onOpenServer(server)}
                      >
                        <span class="h-2 w-2 rounded-full bg-[#53b842]" />
                        <span class="flex min-w-0 flex-col gap-1">
                          <span class="flex min-w-0 items-baseline gap-2 overflow-hidden text-ellipsis whitespace-nowrap font-semibold">
                            {server.name}
                            <span class="text-[0.78rem] text-[var(--faint)]">v{servers.status()?.version}</span>
                          </span>
                          <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.82rem] text-[var(--faint)]">
                            {server.username || "no username"}
                          </span>
                        </span>
                        <Show when={isActiveServer(server.address, servers.activeServerUrl())}>
                          <IconCheck class="h-4 w-4 text-[var(--muted)]" />
                        </Show>
                        <IconDots class="h-4 w-4 text-[var(--faint)]" />
                      </button>
                    )}
                  </For>
                </div>
                <button
                  class="mx-6 mb-6 mt-2 inline-flex min-h-[2.45rem] w-fit flex-none items-center gap-2 rounded-lg border border-[var(--line-strong)] px-3 font-medium text-[var(--text)] hover:bg-white/[0.045]"
                  type="button"
                  onClick={servers.onShowAddServer}
                >
                  <IconPlus class="h-4 w-4" />
                  <span>Add server</span>
                </button>
              </>
            }
          >
            <form class="mx-6 mb-6 grid min-h-0 flex-1 gap-4 overflow-y-auto rounded-[10px] bg-[#181818] p-5" onSubmit={servers.onSaveServer}>
              <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                <span>Server address</span>
                <input
                  class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                  ref={servers.setAddressRef}
                  value={servers.address()}
                  onInput={(event) => servers.setAddress(event.currentTarget.value)}
                  placeholder="http://localhost:4096"
                />
              </label>
              <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                <span>Server name (optional)</span>
                <input
                  class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                  value={servers.name()}
                  onInput={(event) => servers.setName(event.currentTarget.value)}
                  placeholder="Localhost"
                />
              </label>
              <div class="grid grid-cols-2 gap-4 max-[560px]:grid-cols-1">
                <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                  <span>Username (optional)</span>
                  <input
                    class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                    value={servers.username()}
                    onInput={(event) => servers.setUsername(event.currentTarget.value)}
                    placeholder="opencode"
                  />
                </label>
                <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                  <span>Password (optional)</span>
                  <input
                    class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                    type="password"
                    value={servers.password()}
                    onInput={(event) => servers.setPassword(event.currentTarget.value)}
                    placeholder="password"
                  />
                </label>
              </div>
              <button
                class="h-[2.55rem] w-fit rounded-lg bg-[#e5e2dc] px-4 font-bold text-[#171717]"
                type="submit"
                disabled={!servers.address().trim()}
              >
                Add server
              </button>
            </form>
          </Show>
        </section>
      </div>
    </Show>
  )
}

function CountBadge(props: { count: number }) {
  return (
    <span class="rounded-md bg-white/[0.075] px-1.5 py-0.5 text-[0.68rem] font-bold leading-none text-[var(--text)]">
      {props.count}
    </span>
  )
}
