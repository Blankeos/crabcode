import type { PolymorphicProps } from "@kobalte/core/polymorphic"
import * as PopoverPrimitive from "@kobalte/core/popover"
import type { Component, ValidComponent } from "solid-js"
import { splitProps } from "solid-js"

const PopoverTrigger = PopoverPrimitive.Trigger

const Popover: Component<PopoverPrimitive.PopoverRootProps> = (props) => {
  return <PopoverPrimitive.Root gutter={8} {...props} />
}

type PopoverContentProps<T extends ValidComponent = "div"> =
  PopoverPrimitive.PopoverContentProps<T> & {
    class?: string | undefined
  }

const PopoverContent = <T extends ValidComponent = "div">(
  props: PolymorphicProps<T, PopoverContentProps<T>>
) => {
  const [local, others] = splitProps(props as PopoverContentProps, ["class", "onOpenAutoFocus"])
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        class={local.class}
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          local.onOpenAutoFocus?.(event)
        }}
        {...others}
      />
    </PopoverPrimitive.Portal>
  )
}

export { Popover, PopoverContent, PopoverTrigger }
