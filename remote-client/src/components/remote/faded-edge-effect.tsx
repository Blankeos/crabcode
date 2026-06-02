export function FadedEdgeEffect(props: {
  color?: string
  direction?: "vertical" | "horizontal" | "radial" | "top" | "bottom"
  hidden?: boolean
  size?: string
}) {
  const color = () => props.color ?? "var(--bg)"
  const direction = () => props.direction ?? "radial"
  const size = () => props.size ?? "5rem"

  const style = () => {
    switch (direction()) {
      case "horizontal":
        return {
          background: `linear-gradient(90deg, ${color()} 0%, rgba(0,0,0,0) 5%, rgba(0,0,0,0) 95%, ${color()} 100%)`,
        }
      case "vertical":
        return {
          background: `linear-gradient(0deg, ${color()} 0%, rgba(0,0,0,0) 5%, rgba(0,0,0,0) 95%, ${color()} 100%)`,
        }
      case "top":
        return {
          background: `linear-gradient(180deg, ${color()} 0%, rgba(0,0,0,0) 100%)`,
          height: size(),
          bottom: "auto",
        }
      case "bottom":
        return {
          background: `linear-gradient(0deg, ${color()} 0%, rgba(0,0,0,0) 100%)`,
          height: size(),
          top: "auto",
        }
      default:
        return {
          background: `radial-gradient(circle, rgba(2,0,36,0) 0%, rgba(232,232,235,0) 76%, ${color()} 100%)`,
        }
    }
  }

  return (
    <div
      class="pointer-events-none absolute inset-0 z-10 transition-opacity duration-200"
      style={{
        ...style(),
        opacity: props.hidden ? 0 : 1,
      }}
    />
  )
}
