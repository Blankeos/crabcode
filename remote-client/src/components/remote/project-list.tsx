import { type Accessor, createEffect, createSignal, For, Index, onCleanup, onMount } from "solid-js"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "../ui/context-menu"
import { IconCaretDown, IconPlus } from "../../icons"
import type { RemoteSession } from "../../remote-api"
import { cx } from "../../lib/cx"
import { CollapsiblePanel } from "./collapsible-panel"
import { FadedEdgeEffect } from "./faded-edge-effect"
import { ProjectFavicon } from "./project-favicon"

export type ProjectGroup = {
  name: string
  path: string
  sessions: RemoteSession[]
}

type ProjectListProps = {
  projects: Accessor<ProjectGroup[]>
  openProjects: Accessor<Set<string>>
  activeProjectPath: Accessor<string | null | undefined>
  token: Accessor<string>
  currentSessionId: Accessor<string | null | undefined>
  sidebarOpen?: Accessor<boolean>
  onToggleProject: (key: string) => void
  onNewSession: (workspacePath?: string) => void
  onSwitchSession: (id: string) => void
  onArchiveSession: (id: string) => void
  onArchiveProject: (path: string) => void
}

export function ProjectList(props: ProjectListProps) {
  const [scrollEl, setScrollEl] = createSignal<HTMLDivElement>()
  const edges = useScrollEdges(scrollEl)
  let lastScrolledPath = ""

  createEffect(() => {
    if (props.sidebarOpen && !props.sidebarOpen()) return
    const path = props.activeProjectPath()?.trim()
    const projectCount = props.projects().length
    if (!path || projectCount === 0 || path === lastScrolledPath) return
    lastScrolledPath = path
    queueMicrotask(() => {
      window.requestAnimationFrame(() => {
        const root = scrollEl()
        if (!root) return
        const row = root.querySelector<HTMLElement>("[data-active-project='true']")
          ?? root.querySelector<HTMLElement>(`[data-project-path="${cssEscapeAttr(path)}"]`)
        row?.scrollIntoView({ block: "start", behavior: "smooth" })
      })
    })
  })

  return (
    <div class="relative min-h-0 flex-1">
      <div
        ref={setScrollEl}
        class="h-full min-h-0 overflow-y-auto overflow-x-hidden px-3 pb-[65vh]"
      >
        <Index each={props.projects()}>
          {(project) => {
            const key = () => project().path || project().name
            const open = () => props.openProjects().has(key())
            const active = () => isActiveProject(project().path, props.activeProjectPath())
            return (
              <section
                class="min-w-0 scroll-mt-1"
                data-active-project={active() ? "true" : undefined}
                data-project-path={project().path || project().name}
              >
                <div class="sticky top-0 z-20 -mx-3 bg-[var(--panel)] px-3 py-0.5">
                  <ContextMenu>
                    <ContextMenuTrigger as="div">
                      <div
                        class={cx(
                          "grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-1 rounded-lg transition hover:bg-[#242424]",
                          active() && "bg-white/[0.055] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.035)]"
                        )}
                      >
                        <button
                          class="flex min-h-[2.55rem] w-full min-w-0 items-center gap-2 rounded-lg px-2 text-left text-[var(--text)]"
                          type="button"
                          aria-expanded={open()}
                          aria-current={active() ? "page" : undefined}
                          onClick={() => props.onToggleProject(key())}
                        >
                          <IconCaretDown
                            class="h-4 w-4 text-[var(--faint)] transition-transform duration-[180ms]"
                            style={{
                              transform: open() ? "rotate(0deg)" : "rotate(-90deg)",
                            }}
                          />
                          <ProjectFavicon
                            cwd={project().path}
                            label={project().name}
                            token={props.token()}
                          />
                          <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[0.95rem] font-bold">
                            {project().name}
                          </span>
                        </button>
                        <button
                          class="mr-1 grid h-[1.7rem] w-[1.7rem] place-items-center rounded-md text-[var(--faint)] opacity-70 transition hover:bg-[#2d2d2d] hover:text-[var(--text)] hover:opacity-100 focus-visible:bg-[#2d2d2d] focus-visible:text-[var(--text)] focus-visible:opacity-100"
                          type="button"
                          title={`New chat in ${project().name}`}
                          onClick={(event) => {
                            event.stopPropagation()
                            props.onNewSession(project().path)
                          }}
                        >
                          <IconPlus class="h-[0.92rem] w-[0.92rem]" />
                        </button>
                      </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem
                        class="whitespace-nowrap"
                        onSelect={() => props.onNewSession(project().path)}
                      >
                        New chat
                      </ContextMenuItem>
                      <ContextMenuSeparator />
                      <ContextMenuItem
                        class="danger"
                        onSelect={() => props.onArchiveProject(project().path)}
                      >
                        Archive project
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                </div>
                <CollapsiblePanel open={open()} class="w-full">
                  <div class="ml-[1.65rem] overflow-hidden border-l border-white/[0.055]">
                    <For each={project().sessions}>
                      {(session) => (
                        <SessionRow
                          session={session}
                          active={props.currentSessionId() === session.id}
                          onClick={() => props.onSwitchSession(session.id)}
                          onArchive={() => props.onArchiveSession(session.id)}
                        />
                      )}
                    </For>
                  </div>
                </CollapsiblePanel>
              </section>
            )
          }}
        </Index>
      </div>
      <FadedEdgeEffect direction="top" hidden={edges.isAtTop()} size="2.5rem" color="var(--panel)" />
      <FadedEdgeEffect direction="bottom" hidden={edges.isAtBottom()} size="4rem" color="var(--panel)" />
    </div>
  )
}

function isActiveProject(projectPath: string, activeProjectPath: string | null | undefined): boolean {
  const project = projectPath.trim()
  const active = activeProjectPath?.trim()
  return Boolean(project && active && project === active)
}

function cssEscapeAttr(value: string) {
  if (typeof CSS !== "undefined" && "escape" in CSS) {
    return CSS.escape(value)
  }
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')
}

function SessionRow(props: {
  session: RemoteSession
  active: boolean
  onClick: () => void
  onArchive: () => void
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger as="div">
        <button
          class={cx(
            "grid min-h-[2.45rem] w-full min-w-0 grid-cols-[auto_1fr_auto] items-center gap-x-2 rounded-[0_0.5rem_0.5rem_0] py-1 pr-2 pl-2 text-left text-[var(--muted)] transition hover:bg-white/[0.035] hover:text-[var(--text)]",
            props.active && "bg-white/[0.055] text-[var(--text)]"
          )}
          type="button"
          onClick={props.onClick}
        >
          <span class={cx("h-2 w-2 rounded-full", statusClass(props.session.status))} />
          <span class="flex min-w-0 flex-col gap-0.5">
            <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-[0.9rem] font-medium">
              {props.session.title || "Untitled chat"}
            </span>
            <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">
              {statusLabel(props.session.status)}
            </span>
          </span>
          <span class="whitespace-nowrap text-[0.72rem] text-[var(--faint)]">
            {relativeTime(props.session.updated_at)}
          </span>
        </button>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={props.onClick}>Open session</ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem class="danger" onSelect={props.onArchive}>
          Archive session
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

function statusClass(status: string): string {
  if (status === "running" || status === "streaming" || status === "pending") return "bg-[var(--blue)]"
  if (status === "failed" || status === "error" || status === "interrupted") return "bg-[var(--red)]"
  return "bg-[var(--green)]"
}

function statusLabel(status: string): string {
  if (status === "running" || status === "streaming") return "Running"
  if (status === "pending") return "Queued"
  if (status === "failed" || status === "error") return "Failed"
  if (status === "interrupted") return "Interrupted"
  return "Ready"
}

function relativeTime(timestamp: number): string {
  if (!timestamp) return ""
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - timestamp))
  if (seconds < 60) return "now"
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 30) return `${days}d ago`
  const months = Math.floor(days / 30)
  if (months < 12) return `${months}mo ago`
  return `${Math.floor(months / 12)}y ago`
}

function useScrollEdges(scrollEl: Accessor<HTMLElement | undefined>) {
  const [isAtTop, setIsAtTop] = createSignal(true)
  const [isAtBottom, setIsAtBottom] = createSignal(true)

  const update = () => {
    const el = scrollEl()
    if (!el) return
    setIsAtTop(el.scrollTop <= 1)
    setIsAtBottom(el.scrollTop + el.clientHeight >= el.scrollHeight - 1)
  }

  onMount(() => {
    createEffect(() => {
      const el = scrollEl()
      if (!el) return
      update()
      el.addEventListener("scroll", update, { passive: true })
      const resizeObserver = new ResizeObserver(update)
      resizeObserver.observe(el)
      onCleanup(() => {
        el.removeEventListener("scroll", update)
        resizeObserver.disconnect()
      })
    })
  })

  return { isAtTop, isAtBottom, update }
}
