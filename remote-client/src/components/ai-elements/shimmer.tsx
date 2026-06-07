import type { Component, JSX } from "solid-js"
import { Dynamic } from "solid-js/web"
import { cx } from "../../lib/cx"

type ShimmerProps = {
  children: string
  as?: keyof JSX.IntrinsicElements
  class?: string
  duration?: number
  spread?: number
}

export const Shimmer: Component<ShimmerProps> = (props) => {
  const duration = () => props.duration ?? 1.6
  const spread = () => (props.children?.length ?? 0) * (props.spread ?? 2)

  return (
    <Dynamic
      component={props.as ?? "span"}
      class={cx(
        "relative inline-block bg-[length:250%_100%,auto] bg-clip-text text-transparent",
        "[animation:ai-shimmer_var(--shimmer-duration,2s)_linear_infinite]",
        props.class
      )}
      style={{
        "--shimmer-spread": `${spread()}px`,
        "--shimmer-duration": `${duration()}s`,
        "background-image":
          "linear-gradient(90deg, transparent calc(50% - var(--shimmer-spread)), var(--text), transparent calc(50% + var(--shimmer-spread))), linear-gradient(var(--muted), var(--muted))",
        "background-repeat": "no-repeat, padding-box",
      }}
    >
      {props.children}
    </Dynamic>
  )
}
