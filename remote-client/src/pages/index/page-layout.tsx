import { For, Index, Show } from "solid-js"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "cmdk-solid"
import { IconBrainGlyph } from "../../assets/icons"
import { FadedEdgeEffect } from "../../components/remote/faded-edge-effect"
import { ProjectFavicon } from "../../components/remote/project-favicon"
import { ProjectList } from "../../components/remote/project-list"
import { Popover, PopoverContent, PopoverTrigger } from "../../components/ui/popover"
import { IconArrowLeft, IconCaretDown, IconCheck, IconDots, IconFolder, IconPlus, IconSearch, IconServers, IconSidebar, IconX } from "../../icons"
import { cx } from "../../lib/cx"
import { ICON_BUTTON, INPUT_BASE, PANEL_BASE, POPOVER_ANIMATION } from "./page-constants"
import type { CommandPaletteController, HeaderController, PairPanelController, ProjectPathFormController, ProjectPickerController, RemoteClientUi, ServerPanelController, SidebarController, ThreadController } from "./page-types"
import { ComposerDock } from "./composer-dock"
import { EmptyThread } from "./empty-thread"
import { QuestionRequestPanel, PermissionRequestPanel } from "./request-panels"
import { ImagePreviewDialog, ThreadItemView } from "./thread-view"
import { isActiveServer } from "./server-utils"
import { relativeTime } from "./shared-utils"

export function RemoteClientPage(props: { ui: RemoteClientUi }) {
  const ui = props.ui

  return (
    <div
      class={cx(
        "grid h-dvh overflow-hidden bg-[var(--bg)] min-[901px]:grid-cols-[clamp(16.5rem,19vw,20rem)_minmax(0,1fr)] max-[900px]:grid-cols-1"
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

      <main class="relative flex h-dvh min-h-0 min-w-0 flex-col overflow-hidden bg-[#171717]">
        <MainHeader header={ui.header} />
        <ThreadViewport thread={ui.thread} />
        <ComposerDock composer={ui.composer} />
      </main>

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
            class="inline-flex h-[2.2rem] items-center gap-2 rounded-lg border border-[var(--line-strong)] bg-[#222222] px-3 text-[0.86rem] font-semibold text-[#d7d5d0] transition hover:border-[rgba(255,255,255,0.18)] hover:bg-[#2b2b2b] hover:text-[var(--text)] max-[560px]:aspect-square max-[560px]:w-[2.2rem] max-[560px]:justify-center max-[560px]:p-0"
            type="button"
            onClick={() => header.onNewSession()}
          >
            <IconPlus class="h-4 w-4" />
            <span class="max-[560px]:hidden">New chat</span>
          </button>
        </Show>
        <ServerPopover servers={header.servers} />
      </div>
    </header>
  )
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
        class="grid min-w-0 max-w-[min(36rem,52vw)] flex-[0_1_auto] grid-cols-[minmax(0,auto)_auto] items-center justify-start gap-2 rounded-lg px-2 py-1.5 text-left transition hover:bg-white/[0.035]"
        type="button"
      >
        <span class="flex min-w-0 flex-col gap-0.5">
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[1.12rem] font-bold text-[var(--text)]">
            {picker.projectName()}
          </span>
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.72rem] text-[var(--faint)] max-[560px]:hidden">
            {picker.projectPath()}
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
            {(project) => (
              <button
                class={cx(
                  "grid w-full min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-lg p-2 text-left text-[var(--text)] hover:bg-white/[0.055]",
                  project.path === picker.projectPath() && "bg-white/[0.055]"
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
            )}
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
        <span class="absolute top-[0.42rem] right-[0.42rem] h-[0.43rem] w-[0.43rem] rounded-full bg-[#53b842]" />
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
            class={cx("py-1 text-[0.9rem] text-[var(--muted)]", servers.tab() === "mcp" && "text-[var(--text)]")}
            type="button"
            onClick={() => servers.onSelectTab("mcp")}
            disabled
          >
            MCP
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
          "h-full min-h-0 overflow-y-auto overflow-x-hidden px-4 pt-5",
          thread.isEmptyChat() ? "overflow-hidden pb-[clamp(8rem,22vh,12rem)]" : "pb-52"
        )}
      >
        <div ref={thread.setContentRef} class={cx("mx-auto w-[min(100%,64rem)]", thread.isEmptyChat() && "grid h-full")}>
          <Show
            when={thread.visibleMessages().length > 0}
            fallback={
              <EmptyThread
                projectName={thread.projectName()}
                mascotFrame={thread.mascotFrame()}
              />
            }
          >
            <Index each={thread.threadItems()}>
              {(item) => (
                <ThreadItemView
                  item={item}
                  status={thread.status}
                  token={thread.token}
                  onPreviewImage={thread.onPreviewImage}
                />
              )}
            </Index>
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
                    <div>
                      <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">New chat</div>
                      <div class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">Start a blank session in this workspace</div>
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
