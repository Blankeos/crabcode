import { For, Show } from "solid-js"
import { cx } from "../../lib/cx"
import type { RemoteThreadTab } from "../../remote-api"
import type { ThreadTabsController } from "./page-types"
import { agentAccentClass, displayAgentMode } from "./shared-utils"

export function ThreadTabsBar(props: { tabs: ThreadTabsController }) {
  const bundle = () => props.tabs.tabs()
  const show = () => Boolean(bundle()?.tabs.length)

  return (
    <Show when={show()}>
      <div
        class="relative z-[2] shrink-0 border-b border-[var(--line)] bg-[#141414]/95 backdrop-blur-sm"
        role="tablist"
        aria-label="Session threads"
      >
        <div class="mx-auto flex w-[min(100%,64rem)] items-stretch gap-0 overflow-x-auto px-4 [scrollbar-width:thin]">
          <For each={bundle()?.tabs ?? []}>
            {(tab) => (
              <ThreadTabButton
                tab={tab}
                disabled={props.tabs.switching()}
                onSelect={() => props.tabs.onSelectTab(tab.session_id)}
              />
            )}
          </For>
        </div>
      </div>
    </Show>
  )
}

function ThreadTabButton(props: {
  tab: RemoteThreadTab
  disabled: boolean
  onSelect: () => void
}) {
  const tab = () => props.tab
  const isMain = () => tab().kind === "main"
  const label = () => (isMain() ? "Parent" : tab().label)

  return (
    <button
      type="button"
      role="tab"
      aria-selected={tab().active}
      disabled={props.disabled || tab().active}
      class={cx(
        "group relative flex max-w-[14rem] shrink-0 items-center gap-2 border-b-2 px-3 py-2.5 text-left transition",
        "border-transparent text-[var(--muted)] hover:bg-white/[0.04] hover:text-[var(--text)]",
        tab().active && "border-[var(--brand-primary)] bg-white/[0.035] text-[var(--text)]",
        (props.disabled || tab().active) && "cursor-default"
      )}
      style={tab().active ? { "border-bottom-color": tab().accent || undefined } : undefined}
      title={isMain() ? "Parent session" : `${displayAgentMode(tab().agent)} · ${tab().model}`}
      onClick={() => {
        if (props.disabled || tab().active) return
        props.onSelect()
      }}
    >
      <span
        class={cx(
          "grid h-5 min-w-[1.25rem] place-items-center rounded px-1 font-mono text-[0.62rem] font-bold uppercase leading-none",
          isMain() ? "border border-[var(--line-strong)] text-[var(--muted)]" : agentAccentClass(tab().agent)
        )}
      >
        {isMain() ? "P" : tab().label.slice(0, 1).toUpperCase()}
      </span>
      <span class="flex min-w-0 flex-col gap-0.5">
        <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] font-semibold leading-tight">
          {label()}
        </span>
        <Show when={!isMain()}>
          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.62rem] text-[var(--faint)]">
            @{tab().agent}
          </span>
        </Show>
      </span>
      <Show when={tab().running}>
        <span
          class="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--brand-primary)] animate-toolPulse"
          aria-label="Running"
        />
      </Show>
    </button>
  )
}