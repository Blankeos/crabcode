import { createContext, Show, splitProps, useContext, type ComponentProps, type JSX } from "solid-js"
import { IconFileText, IconImage, IconX } from "../../icons"
import { cx } from "../../lib/cx"

export type AttachmentVariant = "grid" | "inline" | "list"

export type AttachmentData = {
  id: string
  url: string
  filename?: string
  mediaType?: string
  size?: number
}

type AttachmentsProps = ComponentProps<"div"> & {
  variant?: AttachmentVariant
  children: JSX.Element
}

type AttachmentProps = ComponentProps<"div"> & {
  data: AttachmentData
  onRemove?: () => void
  children: JSX.Element
}

type AttachmentPreviewProps = ComponentProps<"div"> & {
  fallbackIcon?: JSX.Element
}

type AttachmentInfoProps = ComponentProps<"div"> & {
  showMediaType?: boolean
}

type AttachmentRemoveProps = ComponentProps<"button"> & {
  label?: string
}

type AttachmentsContextValue = {
  variant: AttachmentVariant
}

type AttachmentContextValue = {
  data: AttachmentData
  onRemove?: () => void
}

const AttachmentsContext = createContext<AttachmentsContextValue>()
const AttachmentContext = createContext<AttachmentContextValue>()

export function Attachments(props: AttachmentsProps) {
  const [local, others] = splitProps(props, ["variant", "class", "children"])
  const variant = () => local.variant ?? "grid"

  return (
    <AttachmentsContext.Provider value={{ variant: variant() }}>
      <div
        class={cx(
          variant() === "grid" && "grid grid-cols-[repeat(auto-fill,minmax(7rem,1fr))] gap-2",
          variant() === "inline" && "flex flex-wrap items-center gap-2",
          variant() === "list" && "grid gap-2",
          local.class
        )}
        data-variant={variant()}
        {...others}
      >
        {local.children}
      </div>
    </AttachmentsContext.Provider>
  )
}

export function Attachment(props: AttachmentProps) {
  const attachments = useAttachmentsContext()
  const [local, others] = splitProps(props, ["data", "onRemove", "class", "children"])
  const variant = () => attachments.variant

  return (
    <AttachmentContext.Provider value={{ data: local.data, onRemove: local.onRemove }}>
      <div
        class={cx(
          "group relative min-w-0 overflow-hidden border border-[var(--line)] bg-[#202020] text-[var(--text)]",
          variant() === "grid" && "rounded-[10px]",
          variant() === "inline" && "inline-flex h-10 max-w-full items-center gap-2 rounded-[9px] px-2",
          variant() === "list" && "grid min-h-12 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 rounded-[9px] px-2 py-2",
          local.class
        )}
        data-media-category={getMediaCategory(local.data)}
        {...others}
      >
        {local.children}
      </div>
    </AttachmentContext.Provider>
  )
}

export function AttachmentPreview(props: AttachmentPreviewProps) {
  const attachments = useAttachmentsContext()
  const attachment = useAttachmentContext()
  const [local, others] = splitProps(props, ["class", "fallbackIcon"])
  const variant = () => attachments.variant
  const isImage = () => getMediaCategory(attachment.data) === "image"

  return (
    <div
      class={cx(
        "shrink-0 overflow-hidden bg-[#171717]",
        variant() === "grid" && "aspect-[4/3] w-full",
        variant() === "inline" && "grid h-7 w-7 place-items-center rounded-[6px]",
        variant() === "list" && "grid h-10 w-10 place-items-center rounded-[7px]",
        local.class
      )}
      {...others}
    >
      <Show
        when={isImage()}
        fallback={<AttachmentPreviewIcon class="h-4 w-4 text-[var(--muted)]" fallbackIcon={local.fallbackIcon} />}
      >
        <img
          src={attachment.data.url}
          alt={`Image: ${getAttachmentLabel(attachment.data)}`}
          class={cx("h-full w-full object-cover", variant() !== "grid" && "rounded-[6px]")}
        />
      </Show>
    </div>
  )
}

export function AttachmentInfo(props: AttachmentInfoProps) {
  const attachment = useAttachmentContext()
  const [local, others] = splitProps(props, ["class", "showMediaType"])
  const meta = () =>
    [local.showMediaType ? attachment.data.mediaType : null, attachment.data.size ? formatBytes(attachment.data.size) : null]
      .filter(Boolean)
      .join(" · ")

  return (
    <div class={cx("min-w-0", local.class)} {...others}>
      <div class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.76rem] font-semibold text-[var(--text)]">
        {getAttachmentLabel(attachment.data)}
      </div>
      <div class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.68rem] text-[var(--faint)]">
        {meta()}
      </div>
    </div>
  )
}

export function AttachmentRemove(props: AttachmentRemoveProps) {
  const attachment = useAttachmentContext()
  const [local, others] = splitProps(props, ["class", "children", "label"])

  return (
    <button
      type="button"
      aria-label={local.label ?? "Remove attachment"}
      title={local.label ?? "Remove attachment"}
      class={cx(
        "grid h-7 w-7 place-items-center rounded-md text-[var(--muted)] transition hover:bg-[#2c2c2c] hover:text-[var(--text)] focus-visible:bg-[#2c2c2c] focus-visible:text-[var(--text)]",
        local.class
      )}
      {...others}
      onClick={(event) => {
        event.stopPropagation()
        attachment.onRemove?.()
      }}
    >
      {local.children ?? <IconX class="h-3.5 w-3.5" />}
    </button>
  )
}

export function AttachmentEmpty(props: ComponentProps<"div">) {
  const [local, others] = splitProps(props, ["class", "children"])
  return (
    <div class={cx("text-[0.78rem] text-[var(--faint)]", local.class)} {...others}>
      {local.children ?? "No attachments"}
    </div>
  )
}

export function getMediaCategory(data: AttachmentData) {
  const mediaType = data.mediaType?.toLowerCase() ?? ""
  if (mediaType.startsWith("image/")) return "image"
  if (mediaType.startsWith("video/")) return "video"
  if (mediaType.startsWith("audio/")) return "audio"
  if (mediaType) return "document"
  return "unknown"
}

export function getAttachmentLabel(data: AttachmentData) {
  return data.filename?.trim() || (getMediaCategory(data) === "image" ? "Image" : "Attachment")
}

function AttachmentPreviewIcon(props: { class?: string; fallbackIcon?: JSX.Element }) {
  if (props.fallbackIcon) return <>{props.fallbackIcon}</>
  return getMediaCategory(useAttachmentContext().data) === "image" ? (
    <IconImage class={props.class} />
  ) : (
    <IconFileText class={props.class} />
  )
}

function useAttachmentsContext() {
  const value = useContext(AttachmentsContext)
  if (!value) throw new Error("Attachment components must be used inside <Attachments>")
  return value
}

function useAttachmentContext() {
  const value = useContext(AttachmentContext)
  if (!value) throw new Error("Attachment child components must be used inside <Attachment>")
  return value
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return ""
  if (bytes < 1024) return `${bytes}B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)}KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)}MB`
}
