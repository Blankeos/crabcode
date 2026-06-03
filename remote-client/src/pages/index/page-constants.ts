export const AGENT_MODES = ["Build", "Plan"]
export const MAX_COMPOSER_ATTACHMENTS = 8
export const MAX_COMPOSER_ATTACHMENT_BYTES = 16 * 1024 * 1024
export const IMAGE_FILE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"]
export const MAX_PROMPT_HISTORY = 100
export const MENTION_ACCENTS = [
  { text: "#bfa8ff", background: "rgba(177, 143, 255, 0.14)", ring: "rgba(177, 143, 255, 0.24)" },
  { text: "#8edfc0", background: "rgba(96, 185, 148, 0.13)", ring: "rgba(96, 185, 148, 0.22)" },
  { text: "#f0bd7e", background: "rgba(210, 148, 68, 0.13)", ring: "rgba(210, 148, 68, 0.22)" },
  { text: "#8fc9ff", background: "rgba(92, 158, 219, 0.13)", ring: "rgba(92, 158, 219, 0.22)" },
  { text: "#f1a7bc", background: "rgba(214, 101, 128, 0.13)", ring: "rgba(214, 101, 128, 0.22)" },
  { text: "#d5d985", background: "rgba(177, 184, 82, 0.13)", ring: "rgba(177, 184, 82, 0.22)" },
]
export const PANEL_BASE =
  "rounded-xl border border-[var(--line-strong)] bg-[#202020] shadow-[0_1rem_3rem_rgba(0,0,0,0.35)] outline-none"
export const POPOVER_ANIMATION =
  "origin-[var(--kb-popover-content-transform-origin)] data-[expanded]:animate-flyUpAndScale data-[closed]:animate-flyUpAndScaleExit"
export const ICON_BUTTON =
  "grid place-items-center rounded-md text-[var(--muted)] transition hover:bg-[#282828] hover:text-[var(--text)] focus-visible:bg-[#282828] focus-visible:text-[var(--text)]"
export const MENU_ROW =
  "flex min-h-[2.15rem] w-full items-center justify-between gap-5 rounded-[7px] px-2 text-left text-[0.9rem] text-[var(--muted)] transition hover:bg-[#2b2b2b] hover:text-[var(--text)] focus-visible:bg-[#2b2b2b] focus-visible:text-[var(--text)]"
export const MENU_ROW_ACTIVE =
  "bg-[#2d2d2d] text-[var(--text)] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.035)]"
export const INPUT_BASE =
  "min-w-0 rounded-lg border border-[var(--line)] bg-[#181818] px-3 text-[var(--text)] outline-none"
export const COMPOSER_TEXT_CLASS = "px-5 pt-5 pb-2 text-[0.98rem] leading-normal"
