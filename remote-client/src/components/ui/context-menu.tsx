import * as ContextMenuPrimitive from "@kobalte/core/context-menu"
import type { PolymorphicProps } from "@kobalte/core/polymorphic"
import type { ValidComponent } from "solid-js"
import { splitProps } from "solid-js"
import { cx } from "../../lib/cx"

const ContextMenu = ContextMenuPrimitive.Root
const ContextMenuTrigger = ContextMenuPrimitive.Trigger

type ContextMenuContentProps<T extends ValidComponent = "div"> =
  ContextMenuPrimitive.ContextMenuContentProps<T> & {
    class?: string | undefined
  }

const ContextMenuContent = <T extends ValidComponent = "div">(
  props: PolymorphicProps<T, ContextMenuContentProps<T>>
) => {
  const [local, others] = splitProps(props as ContextMenuContentProps, ["class"])
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        class={cx(
          "z-[130] min-w-40 overflow-hidden rounded-[9px] border border-[var(--line-strong)] bg-[#202020] p-1 text-[var(--text)] shadow-[0_1rem_3rem_rgba(0,0,0,0.38)] outline-none",
          "origin-[var(--kb-menu-content-transform-origin)] data-[expanded]:animate-flyUpAndScale data-[closed]:animate-flyUpAndScaleExit",
          local.class
        )}
        {...others}
      />
    </ContextMenuPrimitive.Portal>
  )
}

type ContextMenuItemProps<T extends ValidComponent = "div"> =
  ContextMenuPrimitive.ContextMenuItemProps<T> & {
    class?: string | undefined
  }

const ContextMenuItem = <T extends ValidComponent = "div">(
  props: PolymorphicProps<T, ContextMenuItemProps<T>>
) => {
  const [local, others] = splitProps(props as ContextMenuItemProps, ["class"])
  const danger = local.class?.split(/\s+/).includes("danger")
  return (
    <ContextMenuPrimitive.Item
      class={cx(
        "flex min-h-[2.05rem] cursor-default select-none items-center rounded-[7px] px-2 text-[0.84rem] text-[var(--muted)] outline-none",
        "hover:bg-[#2b2b2b] hover:text-[var(--text)] focus:bg-[#2b2b2b] focus:text-[var(--text)] data-[highlighted]:bg-[#2b2b2b] data-[highlighted]:text-[var(--text)] data-[focused]:bg-[#2b2b2b] data-[focused]:text-[var(--text)]",
        danger &&
          "text-[#d4929a] hover:bg-[rgba(200,108,116,0.12)] hover:text-[#efb0b7] focus:bg-[rgba(200,108,116,0.12)] focus:text-[#efb0b7] data-[highlighted]:bg-[rgba(200,108,116,0.12)] data-[highlighted]:text-[#efb0b7] data-[focused]:bg-[rgba(200,108,116,0.12)] data-[focused]:text-[#efb0b7]",
        local.class
      )}
      {...others}
    />
  )
}

type ContextMenuSeparatorProps<T extends ValidComponent = "hr"> =
  ContextMenuPrimitive.ContextMenuSeparatorProps<T> & {
    class?: string | undefined
  }

const ContextMenuSeparator = <T extends ValidComponent = "hr">(
  props: PolymorphicProps<T, ContextMenuSeparatorProps<T>>
) => {
  const [local, others] = splitProps(props as ContextMenuSeparatorProps, ["class"])
  return (
    <ContextMenuPrimitive.Separator
      class={cx("my-1 -mx-0.5 h-px border-0 bg-[var(--line)]", local.class)}
      {...others}
    />
  )
}

export { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger }
