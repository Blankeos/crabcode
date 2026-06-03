import { useHotkeys } from "bagon-hooks"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "cmdk-solid"
import { type Accessor, createEffect, createMemo, createSignal, For, Index, type JSX, onCleanup, onMount, type Setter, Show } from "solid-js"
import { StreamMarkdown } from "solid-streamdown"
import "solid-streamdown/styles.css"
import { toast } from "solid-sonner"
import crabcodeLogo from "../../../../crabcode-logo.txt?raw"
import mascotArt from "../../../../mascot.txt?raw"
import {
  IconBrainGlyph,
  IconIconFileCss,
  IconIconFileDefault,
  IconIconFileHtml,
  IconIconFileJs,
  IconIconFileJson,
  IconIconFileMarkdown,
  IconIconFileRust,
  IconIconFileToml,
  IconIconFileTs,
  IconIconFileTsx,
  IconIconFileYaml,
} from "../../assets/icons"
import {
  Attachment,
  AttachmentInfo,
  AttachmentPreview,
  AttachmentRemove,
  Attachments,
  type AttachmentData,
} from "../../components/ai-elements/attachments"
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
  MessageResponse,
  MessageToolbar,
} from "../../components/ai-elements/message"
import { Shimmer } from "../../components/ai-elements/shimmer"
import { CollapsiblePanel } from "../../components/remote/collapsible-panel"
import { FadedEdgeEffect } from "../../components/remote/faded-edge-effect"
import { ProjectFavicon } from "../../components/remote/project-favicon"
import { ProjectList, type ProjectGroup } from "../../components/remote/project-list"
import { Popover, PopoverContent, PopoverTrigger } from "../../components/ui/popover"
import {
  IconArrowLeft,
  IconArrowUp,
  IconCaretDown,
  IconCheck,
  IconCopy,
  IconDots,
  IconFileText,
  IconFolder,
  IconGlobe,
  IconPaperclip,
  IconPencilSimple,
  IconPlus,
  IconSearch,
  IconServers,
  IconSidebar,
  IconTerminal,
  IconWarningCircle,
  IconX,
} from "../../icons"
import { cx } from "../../lib/cx"
import {
  createRemoteApi,
  RemoteApiError,
  type RemoteMessage,
  type RemoteMessagePart,
  type RemoteModel,
  type RemotePendingPermission,
  type RemotePendingQuestion,
  type RemotePromptImage,
  type RemoteQuestionItem,
  type RemoteSkill,
  type RemoteStatus,
  type RemoteState,
  type RemoteSuggestion,
} from "../../remote-api"
import "../../styles/app.css"

const TOKEN_KEY = "crabcode.remote.token"
const SERVERS_KEY = "crabcode.remote.servers"
const PROMPT_HISTORY_KEY = "crabcode.remote.promptHistory"
const AGENT_MODES = ["Build", "Plan"]
const MAX_COMPOSER_ATTACHMENTS = 8
const MAX_COMPOSER_ATTACHMENT_BYTES = 16 * 1024 * 1024
const IMAGE_FILE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"]
const MAX_PROMPT_HISTORY = 100
const MENTION_ACCENTS = [
  { text: "#bfa8ff", background: "rgba(177, 143, 255, 0.14)", ring: "rgba(177, 143, 255, 0.24)" },
  { text: "#8edfc0", background: "rgba(96, 185, 148, 0.13)", ring: "rgba(96, 185, 148, 0.22)" },
  { text: "#f0bd7e", background: "rgba(210, 148, 68, 0.13)", ring: "rgba(210, 148, 68, 0.22)" },
  { text: "#8fc9ff", background: "rgba(92, 158, 219, 0.13)", ring: "rgba(92, 158, 219, 0.22)" },
  { text: "#f1a7bc", background: "rgba(214, 101, 128, 0.13)", ring: "rgba(214, 101, 128, 0.22)" },
  { text: "#d5d985", background: "rgba(177, 184, 82, 0.13)", ring: "rgba(177, 184, 82, 0.22)" },
]
const LOGO_ART = normalizeArt(crabcodeLogo, { trimCommonIndent: true })
const MASCOT_FRAMES = mascotArt
  .trimEnd()
  .split(/\n\s*\n/)
  .filter((frame) => frame.trim().length > 0)
  .map((frame) => normalizeArt(frame))

type SavedServer = {
  id: string
  address: string
  name: string
  username: string
  password: string
}

type RemotePermissionResponse = "deny" | "allow_once" | "allow_always"

type ServerPanelTab = "servers" | "skills" | "mcp" | "lsp" | "plugins"

type CompletionTrigger = {
  kind: "slash" | "mention"
  query: string
  range: [number, number]
}

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
type JsonObject = { [key: string]: JsonValue }

type ParsedToolMessage = {
  id: string
  name: string
  status: string
  args?: JsonValue
  metadata?: JsonValue
  outputPreview?: string
  title?: string
  lineCount?: number
}

type ToolMessage = {
  message: RemoteMessage
  parsed: ParsedToolMessage
  cwd: string
}

type ThreadItem =
  | { type: "message"; message: RemoteMessage; activityTools: ToolMessage[] }
  | { type: "activity"; tools: ToolMessage[] }
  | { type: "action"; tool: ToolMessage }

type ComposerAttachment = {
  id: string
  name: string
  mediaType: string
  size: number
  dataUrl: string
}

type ImagePreviewTarget = {
  url: string
  label: string
}

type PromptTextPart = {
  kind: "text" | "image" | "mention"
  text: string
}

type ImagePlaceholderRange = {
  number: number
  start: number
  end: number
}

type ToolVisualState = "active" | "complete" | "error"
type ToolIconKind = "brain" | "check" | "file" | "globe" | "pencil" | "search" | "terminal" | "warning"

type ToolStepDetail = {
  label: string
  detail?: string
  status?: ToolVisualState
}

type ToolActivityStep = {
  key: string
  label: string
  icon: ToolIconKind
  state: ToolVisualState
  details: ToolStepDetail[]
  preview?: string
  defaultOpen?: boolean
}

type DiffLine = {
  kind: "add" | "remove" | "context"
  text: string
}

type ActionDescriptor = {
  label: string
  description: string
  state: ToolVisualState
  icon: ToolIconKind
  stats?: { added: number; removed: number }
  details: ToolStepDetail[]
  diffLines: DiffLine[]
  preview?: string
}

const THINKING_TOOL_NAMES = new Set([
  "glob",
  "grep",
  "list",
  "question",
  "read",
  "skill",
  "task",
  "todowrite",
  "update_plan",
  "view_image",
  "webfetch",
])
const EXPLORATION_TOOL_NAMES = new Set(["glob", "grep", "list", "read"])
const ACTION_TOOL_NAMES = new Set(["apply_patch", "bash", "edit", "write"])
const PANEL_BASE =
  "rounded-xl border border-[var(--line-strong)] bg-[#202020] shadow-[0_1rem_3rem_rgba(0,0,0,0.35)] outline-none"
const POPOVER_ANIMATION =
  "origin-[var(--kb-popover-content-transform-origin)] data-[expanded]:animate-flyUpAndScale data-[closed]:animate-flyUpAndScaleExit"
const ICON_BUTTON =
  "grid place-items-center rounded-md text-[var(--muted)] transition hover:bg-[#282828] hover:text-[var(--text)] focus-visible:bg-[#282828] focus-visible:text-[var(--text)]"
const MENU_ROW =
  "flex min-h-[2.15rem] w-full items-center justify-between gap-5 rounded-[7px] px-2 text-left text-[0.9rem] text-[var(--muted)] transition hover:bg-[#2b2b2b] hover:text-[var(--text)] focus-visible:bg-[#2b2b2b] focus-visible:text-[var(--text)]"
const MENU_ROW_ACTIVE =
  "bg-[#2d2d2d] text-[var(--text)] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.035)]"
const INPUT_BASE =
  "min-w-0 rounded-lg border border-[var(--line)] bg-[#181818] px-3 text-[var(--text)] outline-none"
const COMPOSER_TEXT_CLASS = "px-5 pt-5 pb-2 text-[0.98rem] leading-normal"

export default function RemoteClient() {
  const [token, setToken] = createSignal(localStorage.getItem(TOKEN_KEY) || "")
  const api = createMemo(() => createRemoteApi(token))

  const [state, setState] = createSignal<RemoteState | null>(null)
  const [pairRequired, setPairRequired] = createSignal(false)
  const [pairCode, setPairCode] = createSignal("")
  const [pairError, setPairError] = createSignal("")
  const [permissionBusy, setPermissionBusy] = createSignal(false)
  const [questionBusy, setQuestionBusy] = createSignal(false)
  const [sidebarOpen, setSidebarOpen] = createSignal(false)
  const [projectOpen, setProjectOpen] = createSignal<Set<string>>(new Set())
  const [projectsInitialized, setProjectsInitialized] = createSignal(false)
  const [projectPickerOpen, setProjectPickerOpen] = createSignal(false)
  const [projectPickerAddOpen, setProjectPickerAddOpen] = createSignal(false)
  const [newProjectOpen, setNewProjectOpen] = createSignal(false)
  const [projectPathInput, setProjectPathInput] = createSignal("")
  const [projectPathError, setProjectPathError] = createSignal("")
  const [serversOpen, setServersOpen] = createSignal(false)
  const [serverPanelTab, setServerPanelTab] = createSignal<ServerPanelTab>("servers")
  const [serversManageOpen, setServersManageOpen] = createSignal(false)
  const [serverAddOpen, setServerAddOpen] = createSignal(false)
  const [serverSearch, setServerSearch] = createSignal("")
  const [serverAddress, setServerAddress] = createSignal("")
  const [serverName, setServerName] = createSignal("")
  const [serverUsername, setServerUsername] = createSignal("")
  const [serverPassword, setServerPassword] = createSignal("")
  const [savedServers, setSavedServers] = createSignal<SavedServer[]>(loadSavedServers())
  const [agentOpen, setAgentOpen] = createSignal(false)
  const [reasoningOpen, setReasoningOpen] = createSignal(false)
  const [modelOpen, setModelOpen] = createSignal(false)
  const [models, setModels] = createSignal<RemoteModel[]>([])
  const [skills, setSkills] = createSignal<RemoteSkill[]>([])
  const [modelQuery, setModelQuery] = createSignal("")
  const [modelActiveIndex, setModelActiveIndex] = createSignal(0)
  const [agentActiveIndex, setAgentActiveIndex] = createSignal(0)
  const [reasoningActiveIndex, setReasoningActiveIndex] = createSignal(0)
  const [commandRendered, setCommandRendered] = createSignal(false)
  const [commandClosing, setCommandClosing] = createSignal(false)
  const [commandQuery, setCommandQuery] = createSignal("")
  const [prompt, setPrompt] = createSignal("")
  const [composerAttachments, setComposerAttachments] = createSignal<ComposerAttachment[]>([])
  const [imagePreview, setImagePreview] = createSignal<ImagePreviewTarget | null>(null)
  const [browserPromptHistory, setBrowserPromptHistory] = createSignal<string[]>(loadPromptHistory())
  const [promptHistoryIndex, setPromptHistoryIndex] = createSignal<number | null>(null)
  const [promptHistoryDraft, setPromptHistoryDraft] = createSignal("")
  const [composerSuggestions, setComposerSuggestions] = createSignal<RemoteSuggestion[]>([])
  const [composerSuggestionIndex, setComposerSuggestionIndex] = createSignal(0)
  const [completionTrigger, setCompletionTrigger] = createSignal<CompletionTrigger | null>(null)
  const [completionRevision, setCompletionRevision] = createSignal(0)
  const [mascotFrame, setMascotFrame] = createSignal(0)
  const [threadScrollEl, setThreadScrollEl] = createSignal<HTMLDivElement>()
  const [threadContentEl, setThreadContentEl] = createSignal<HTMLDivElement>()
  const threadScroll = useStickToBottom(threadScrollEl, threadContentEl)

  let promptRef: HTMLTextAreaElement | undefined
  let promptOverlayRef: HTMLDivElement | undefined
  let composerSuggestionsRef: HTMLDivElement | undefined
  let imageInputRef: HTMLInputElement | undefined
  let commandInputRef: HTMLInputElement | undefined
  let projectPathInputRef: HTMLInputElement | undefined
  let serverAddressRef: HTMLInputElement | undefined
  let modelSearchRef: HTMLInputElement | undefined
  let focusPromptAfterControlPopoverClose = false
  let closeStateEvents: (() => void) | undefined
  let commandCloseTimer: number | undefined

  const openCommandPalette = () => {
    if (commandCloseTimer !== undefined) {
      window.clearTimeout(commandCloseTimer)
      commandCloseTimer = undefined
    }
    setCommandRendered(true)
    setCommandClosing(false)
    queueMicrotask(() => commandInputRef?.focus())
  }

  const closeCommandPalette = () => {
    if (!commandRendered() || commandClosing()) return
    setCommandClosing(true)
    commandCloseTimer = window.setTimeout(() => {
      setCommandRendered(false)
      setCommandClosing(false)
      commandCloseTimer = undefined
    }, 180)
  }

  useHotkeys(
    [
      [
        "mod+K",
        (event) => {
          event.preventDefault()
          openCommandPalette()
        },
        { preventDefault: true },
      ],
    ],
    []
  )

  createEffect(() => {
    if (!commandRendered()) return

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      closeCommandPalette()
    }

    window.addEventListener("keydown", onKeyDown)
    onCleanup(() => window.removeEventListener("keydown", onKeyDown))
  })

  let completionRequestId = 0
  let completionResultsKey = ""
  createEffect(() => {
    const trigger = completionTrigger()
    completionRevision()
    if (!trigger) {
      setComposerSuggestions([])
      setComposerSuggestionIndex(0)
      completionResultsKey = ""
      return
    }

    const requestId = ++completionRequestId
    const resultsKey = `${trigger.kind}:${trigger.range[0]}:${trigger.query}`
    const resetSelection = resultsKey !== completionResultsKey
    completionResultsKey = resultsKey
    void api()
      .autocomplete(trigger.kind, trigger.query, Boolean(state()?.current_session_id))
      .then((suggestions) => {
        if (requestId !== completionRequestId) return
        const next = suggestions.slice(0, 12)
        setComposerSuggestions(next)
        setComposerSuggestionIndex((index) => (resetSelection ? 0 : Math.min(index, Math.max(next.length - 1, 0))))
      })
      .catch(() => {
        if (requestId !== completionRequestId) return
        setComposerSuggestions([])
        setComposerSuggestionIndex(0)
      })
  })

  createEffect(() => {
    const index = composerSuggestionIndex()
    composerSuggestions().length
    const list = composerSuggestionsRef
    if (!list) return

    queueMicrotask(() => {
      const option = list.querySelector<HTMLElement>(`[data-composer-suggestion-index="${index}"]`)
      option?.scrollIntoView({ block: "nearest" })
    })
  })

  const applyRemoteState = (next: RemoteState) => {
    setState(next)
    if (!projectsInitialized()) {
      setProjectOpen(new Set(projectsFromState(next).map((project) => project.path || project.name)))
      setProjectsInitialized(true)
    }
  }

  const loadStateSnapshot = async () => {
    applyRemoteState(await api().state())
  }

  const openStateEvents = () => {
    closeStateEvents?.()
    closeStateEvents = api().stateEvents(
      (next) => {
        setPairRequired(false)
        setPairError("")
        applyRemoteState(next)
      },
      () => {
        const message = "Live connection interrupted. Reconnecting..."
        setPairError(message)
        toast.error(message)
      }
    )
  }

  const connect = async () => {
    try {
      const status = await api().status()
      if (status.auth_required && !token()) {
        setPairRequired(true)
        return
      }

      const next = await api().state()
      setPairRequired(false)
      applyRemoteState(next)
      openStateEvents()
    } catch (error) {
      if (error instanceof RemoteApiError && error.status === 401) {
        localStorage.removeItem(TOKEN_KEY)
        setToken("")
        closeStateEvents?.()
        closeStateEvents = undefined
        setPairRequired(true)
        return
      }
      setPairRequired(true)
      const message = errorToastMessage(error, "Host unavailable or pairing required.")
      setPairError(message)
      toast.error(message)
    }
  }

  onMount(() => {
    connect()
    const mascotTimer = window.setInterval(() => {
      setMascotFrame((current) => (current + 1) % Math.max(MASCOT_FRAMES.length, 1))
    }, 620)
    onCleanup(() => {
      closeStateEvents?.()
      if (commandCloseTimer !== undefined) window.clearTimeout(commandCloseTimer)
      window.clearInterval(mascotTimer)
    })
  })

  const projectPath = createMemo(() => state()?.status.cwd || "")
  const projectName = createMemo(
    () => state()?.status.workspace || basename(projectPath()) || "Project"
  )
  const projects = createMemo(() => projectsFromState(state()))
  const activeServerUrl = createMemo(() => state()?.status.browser_url || browserOrigin())
  const servers = createMemo(() => {
    const activeUrl = activeServerUrl()
    const seen = new Set<string>()
    const activeServer: SavedServer = {
      id: "active",
      address: activeUrl,
      name: activeUrl.replace(/^https?:\/\//, ""),
      username: "",
      password: "",
    }
    return [activeServer, ...savedServers()].filter((server) => {
      const key = normalizeServerAddress(server.address)
      if (seen.has(key)) return false
      seen.add(key)
      return true
    })
  })
  const filteredServers = createMemo(() => {
    const query = serverSearch().trim().toLowerCase()
    if (!query) return servers()
    return servers().filter((server) =>
      `${server.name} ${server.address} ${server.username}`.toLowerCase().includes(query)
    )
  })
  const reasoningOptions = createMemo(() => state()?.status.reasoning_efforts ?? [])
  const reasoningLabel = createMemo(() => state()?.status.reasoning_effort || "off")
  const pendingPermission = createMemo(() => state()?.pending_permission ?? null)
  const pendingQuestion = createMemo(() => state()?.pending_question ?? null)

  const commandResults = createMemo(() => {
    const query = commandQuery().trim().toLowerCase()
    const sessions = state()?.sessions ?? []
    if (!query) return sessions.slice(0, 12)
    return sessions
      .filter((session) =>
        `${session.workspace} ${session.title} ${session.status}`.toLowerCase().includes(query)
      )
      .slice(0, 24)
  })
  const projectCommandResults = createMemo(() => {
    const query = commandQuery().trim().toLowerCase()
    const list = projects()
    if (!query) return list.slice(0, 8)
    return list
      .filter((project) => `${project.name} ${project.path}`.toLowerCase().includes(query))
      .slice(0, 16)
  })

  const filteredModels = createMemo(() => {
    const query = modelQuery().trim().toLowerCase()
    if (!query) return models()
    return models().filter((model) =>
      `${model.group} ${model.name} ${model.provider_id} ${model.id} ${model.description}`
        .toLowerCase()
        .includes(query)
    )
  })

  createEffect(() => {
    const list = filteredModels()
    if (!modelOpen()) return
    const active = list.findIndex((model) => model.active)
    setModelActiveIndex((index) => Math.max(0, Math.min(index || Math.max(active, 0), Math.max(list.length - 1, 0))))
  })

  const visibleMessages = createMemo(() =>
    (state()?.messages ?? []).filter((message) => message.role !== "system")
  )
  const promptHistoryEntries = createMemo(() =>
    mergePromptHistoryEntries(browserPromptHistory(), messagePromptHistoryEntries(visibleMessages()))
  )
  const currentSession = createMemo(() =>
    (state()?.sessions ?? []).find((session) => session.id === state()?.current_session_id)
  )
  const threadItems = createMemo(() => buildThreadItems(visibleMessages(), projectPath()))
  const isEmptyChat = createMemo(() => threadItems().length === 0 && !state()?.is_streaming)

  createEffect(() => {
    state()?.current_session_id
    queueMicrotask(() => threadScroll.scrollToBottom(false))
  })

  const pair = async (event: SubmitEvent) => {
    event.preventDefault()
    setPairError("")
    try {
      const response = await api().pair(pairCode())
      localStorage.setItem(TOKEN_KEY, response.token)
      setToken(response.token)
      setPairCode("")
      setPairRequired(false)
      await connect()
    } catch (error) {
      const message = errorToastMessage(error, "Pair code rejected.")
      setPairError(message)
      toast.error(message)
    }
  }

  const startNewSession = async (workspacePath?: string) => {
    closeCommandPalette()
    try {
      const next = await api().newSession(workspacePath)
      applyRemoteState(next)
      setSidebarOpen(false)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not start a new chat.")
    }
  }

  const selectWorkspace = async (path: string) => {
    const nextPath = path.trim()
    if (!nextPath) return

    setProjectPathError("")
    try {
      const next = await api().selectWorkspace(nextPath)
      applyRemoteState(next)
      setProjectPickerOpen(false)
      setNewProjectOpen(false)
      setSidebarOpen(false)
      promptRef?.focus()
    } catch (error) {
      const message = errorToastMessage(error, "Could not open folder")
      setProjectPathError(message)
      toast.error(message)
    }
  }

  const submitProjectPath = async (event: SubmitEvent) => {
    event.preventDefault()
    await selectWorkspace(projectPathInput())
  }

  const switchSession = async (id: string) => {
    closeCommandPalette()
    try {
      const next = await api().switchSession(id)
      applyRemoteState(next)
      setSidebarOpen(false)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not switch chat.")
    }
  }

  const archiveSession = async (id: string) => {
    closeCommandPalette()
    try {
      const next = await api().archiveSession(id)
      applyRemoteState(next)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not archive chat.")
    }
  }

  const archiveProject = async (path: string) => {
    const nextPath = path.trim()
    if (!nextPath) return

    closeCommandPalette()
    try {
      const next = await api().archiveWorkspace(nextPath)
      applyRemoteState(next)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not archive project.")
    }
  }

  const copySessionTranscript = async () => {
    const current = currentSession()
    const transcript = sessionTranscript(current?.title || "Untitled", state()?.messages ?? [])
    if (!transcript.trim()) {
      toast.warning("No transcript to copy.")
      return
    }

    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(transcript)
      } else {
        fallbackCopyText(transcript)
      }
      toast.success("Session transcript copied.")
    } catch {
      try {
        fallbackCopyText(transcript)
        toast.success("Session transcript copied.")
      } catch {
        toast.error("Clipboard access was denied.")
      }
    }
  }

  const handleLocalSlashCommand = async (text: string) => {
    const parsed = parseSlashCommand(text)
    if (!parsed) return false

    if (sameToken(parsed.name, "copy")) {
      if (parsed.args.trim()) {
        toast.error("Usage: /copy")
      } else {
        await copySessionTranscript()
      }
      return true
    }

    if (sameToken(parsed.name, "models")) {
      setModelQuery(parsed.args.trim())
      setModelOpen(true)
      void loadModels()
      focusModelSearch()
      return true
    }

    return false
  }

  const composerAttachmentData = createMemo<AttachmentData[]>(() =>
    composerAttachments().map((attachment, index) => ({
      id: attachment.id,
      url: attachment.dataUrl,
      filename: `[Image #${index + 1}] ${attachment.name}`,
      mediaType: attachment.mediaType,
      size: attachment.size,
    }))
  )

  const appendImagePlaceholders = (startIndex: number, count: number) => {
    if (count <= 0) return
    resetPromptHistoryNavigation()
    const placeholders = Array.from({ length: count }, (_, index) => `[Image #${startIndex + index}]`)
    setPrompt((current) => {
      const separator = current.length > 0 && !/\s$/.test(current) ? " " : ""
      return `${current}${separator}${placeholders.join(" ")} `
    })
    setComposerSuggestions([])
    setCompletionTrigger(null)
    queueMicrotask(() => {
      promptRef?.focus()
      resizePrompt()
    })
  }

  const addImageFiles = async (files: File[]) => {
    const imageFiles = files.filter((file) => isSupportedImageFile(file))
    if (imageFiles.length === 0) {
      toast.warning("Paste or choose PNG, JPEG, GIF, or WebP images.")
      return
    }

    const available = MAX_COMPOSER_ATTACHMENTS - composerAttachments().length
    if (available <= 0) {
      toast.warning(`Attach up to ${MAX_COMPOSER_ATTACHMENTS} images.`)
      return
    }

    const accepted = imageFiles.slice(0, available)
    if (imageFiles.length > accepted.length) {
      toast.warning(`Only ${MAX_COMPOSER_ATTACHMENTS} images can be attached.`)
    }

    const oversized = accepted.find((file) => file.size > MAX_COMPOSER_ATTACHMENT_BYTES)
    if (oversized) {
      toast.error(`${oversized.name || "Image"} is larger than 16MB.`)
      return
    }

    try {
      const next = await Promise.all(accepted.map(readComposerAttachment))
      const startIndex = composerAttachments().length + 1
      setComposerAttachments((current) => [...current, ...next])
      appendImagePlaceholders(startIndex, next.length)
      toast.success(next.length === 1 ? "Image attached." : `${next.length} images attached.`)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Could not read image.")
    }
  }

  const handlePromptPaste = (event: ClipboardEvent & { currentTarget: HTMLTextAreaElement }) => {
    const files = filesFromClipboard(event.clipboardData)
    if (files.length === 0) return

    event.preventDefault()
    void addImageFiles(files)
  }

  const handleComposerDrop = (event: DragEvent & { currentTarget: HTMLFormElement }) => {
    const files = Array.from(event.dataTransfer?.files ?? [])
    if (files.length === 0) return

    event.preventDefault()
    void addImageFiles(files)
  }

  const resetPromptHistoryNavigation = () => {
    setPromptHistoryIndex(null)
    setPromptHistoryDraft("")
  }

  const addPromptHistoryEntry = (text: string) => {
    const entry = normalizePromptHistoryEntry(text)
    if (!entry || parseSlashCommand(entry)) return

    setBrowserPromptHistory((current) => {
      const next = [entry, ...current.filter((item) => item !== entry)].slice(0, MAX_PROMPT_HISTORY)
      savePromptHistory(next)
      return next
    })
    resetPromptHistoryNavigation()
  }

  const applyPromptHistoryEntry = (text: string, cursor: "start" | "end") => {
    setPrompt(text)
    setComposerSuggestions([])
    setCompletionTrigger(null)
    queueMicrotask(() => {
      const offset = cursor === "start" ? 0 : text.length
      promptRef?.focus()
      promptRef?.setSelectionRange(offset, offset)
      resizePrompt()
    })
  }

  const navigatePromptHistory = (
    direction: "up" | "down",
    event: KeyboardEvent & { currentTarget: HTMLTextAreaElement }
  ) => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false
    if (event.currentTarget.selectionStart !== event.currentTarget.selectionEnd) return false
    if (composerAttachments().length > 0) return false

    const entries = promptHistoryEntries()
    if (entries.length === 0) return false

    const text = prompt()
    const cursor = event.currentTarget.selectionStart

    if (direction === "up") {
      if (!isCursorOnFirstLogicalLine(text, cursor)) return false

      const currentIndex = promptHistoryIndex()
      const nextIndex = currentIndex == null ? 0 : Math.min(currentIndex + 1, entries.length - 1)
      if (currentIndex === nextIndex) return false

      event.preventDefault()
      if (currentIndex == null) setPromptHistoryDraft(text)
      setPromptHistoryIndex(nextIndex)
      applyPromptHistoryEntry(entries[nextIndex] ?? "", "start")
      return true
    }

    if (!isCursorOnLastLogicalLine(text, cursor)) return false

    const currentIndex = promptHistoryIndex()
    if (currentIndex == null) return false

    event.preventDefault()
    if (currentIndex === 0) {
      const draft = promptHistoryDraft()
      resetPromptHistoryNavigation()
      applyPromptHistoryEntry(draft, "end")
      return true
    }

    const nextIndex = currentIndex - 1
    setPromptHistoryIndex(nextIndex)
    applyPromptHistoryEntry(entries[nextIndex] ?? "", "end")
    return true
  }

  const removeComposerAttachment = (id: string) => {
    const current = composerAttachments()
    const index = current.findIndex((attachment) => attachment.id === id)
    if (index < 0) return

    removeComposerAttachmentNumbers([index + 1])
  }

  const removeComposerAttachmentNumbers = (numbers: number[], text = prompt()) => {
    resetPromptHistoryNavigation()
    const numberSet = new Set(numbers)
    const current = composerAttachments()
    const nextAttachments = current.filter((_, index) => !numberSet.has(index + 1))
    setComposerAttachments(nextAttachments)
    setPrompt(renumberImagePlaceholdersAfterRemoval(text, numbers, current.length))
    setComposerSuggestions([])
    setCompletionTrigger(null)
    queueMicrotask(resizePrompt)
  }

  const syncComposerAttachmentsToText = (text: string) => {
    const current = composerAttachments()
    if (current.length === 0) return text

    const referenced = new Set(
      imagePlaceholderRanges(text)
        .map((range) => range.number)
        .filter((number) => number >= 1 && number <= current.length)
    )
    const removedNumbers = current
      .map((_, index) => index + 1)
      .filter((number) => !referenced.has(number))

    if (removedNumbers.length === 0) return text

    const nextAttachments = current.filter((_, index) => !removedNumbers.includes(index + 1))
    setComposerAttachments(nextAttachments)
    return renumberImagePlaceholdersAfterRemoval(text, removedNumbers, current.length)
  }

  const handlePromptInput = (event: InputEvent & { currentTarget: HTMLTextAreaElement }) => {
    resetPromptHistoryNavigation()
    const cursor = event.currentTarget.selectionStart
    const nextText = syncComposerAttachmentsToText(event.currentTarget.value)
    setPrompt(nextText)
    setCompletionTrigger(detectCompletionTrigger(nextText, Math.min(cursor, nextText.length)))
    setCompletionRevision((revision) => revision + 1)
    resizePrompt()

    if (nextText !== event.currentTarget.value) {
      const nextCursor = Math.min(cursor, nextText.length)
      queueMicrotask(() => {
        promptRef?.setSelectionRange(nextCursor, nextCursor)
        resizePrompt()
      })
    }
  }

  const handlePromptScroll = (event: Event & { currentTarget: HTMLTextAreaElement }) => {
    if (promptOverlayRef) promptOverlayRef.scrollTop = event.currentTarget.scrollTop
  }

  const removePromptImageTagAtCursor = (event: KeyboardEvent & { currentTarget: HTMLTextAreaElement }) => {
    if (event.key !== "Backspace" && event.key !== "Delete") return false
    if (composerAttachments().length === 0) return false

    const textarea = event.currentTarget
    const text = textarea.value
    const selectionStart = textarea.selectionStart
    const selectionEnd = textarea.selectionEnd
    const ranges = imagePlaceholderRanges(text)
    const targetRanges =
      selectionStart !== selectionEnd
        ? ranges.filter((range) => rangesIntersect(range.start, range.end, selectionStart, selectionEnd))
        : ranges.filter((range) =>
            event.key === "Backspace"
              ? (selectionStart > range.start && selectionStart <= range.end) ||
                (selectionStart === range.end + 1 && /\s/.test(text[range.end] ?? ""))
              : selectionStart >= range.start && selectionStart < range.end
          )

    if (targetRanges.length === 0) return false

    event.preventDefault()
    const removeSelection = selectionStart !== selectionEnd
    const removedNumbers = [...new Set(targetRanges.map((range) => range.number))]
    const removalRanges = removeSelection
      ? [...targetRanges, { number: 0, start: selectionStart, end: selectionEnd }]
      : targetRanges
    const cursor = Math.min(...removalRanges.map((range) => range.start))
    const nextText = removeRangesFromText(text, removalRanges)

    removeComposerAttachmentNumbers(removedNumbers, nextText)
    queueMicrotask(() => {
      const nextCursor = Math.min(cursor, prompt().length)
      promptRef?.focus()
      promptRef?.setSelectionRange(nextCursor, nextCursor)
      resizePrompt()
    })
    return true
  }

  const promptImages = (attachments: ComposerAttachment[]): RemotePromptImage[] =>
    attachments.map((attachment) => ({
      name: attachment.name,
      media_type: attachment.mediaType,
      data_url: attachment.dataUrl,
    }))

  const openImagePreview = (attachment: AttachmentData) => {
    const target = imagePreviewFromAttachment(attachment)
    if (target) setImagePreview(target)
  }

  const clearComposer = () => {
    setPrompt("")
    setComposerAttachments([])
    setComposerSuggestions([])
    setCompletionTrigger(null)
    resetPromptHistoryNavigation()
    resizePrompt()
  }

  const submitPromptText = async (
    rawText: string,
    restoreOnError = true,
    attachments = composerAttachments()
  ) => {
    const text = promptTextWithAttachmentPlaceholders(rawText, attachments.length).trim()
    if (!text && attachments.length === 0) return
    if (attachments.length === 0 && await handleLocalSlashCommand(text)) {
      clearComposer()
      return
    }
    if (attachments.length > 0 && parseSlashCommand(text)) {
      toast.error("Images can only be attached to chat prompts.")
      return
    }

    clearComposer()
    try {
      await api().prompt(text, promptImages(attachments))
      addPromptHistoryEntry(text)
      await loadStateSnapshot()
    } catch (error) {
      if (restoreOnError) {
        setPrompt(text)
        setComposerAttachments(attachments)
        resizePrompt()
      }
      toast.error(error instanceof Error ? error.message : "Prompt failed.")
    }
  }

  const submitPrompt = async (event: SubmitEvent) => {
    event.preventDefault()

    if (state()?.is_streaming) {
      try {
        await api().cancel()
        await loadStateSnapshot()
      } catch (error) {
        showErrorToast(error, "Could not stop generation.")
      }
      return
    }

    await submitPromptText(prompt(), true)
  }

  const answerPermission = async (response: RemotePermissionResponse) => {
    if (permissionBusy()) return
    setPermissionBusy(true)
    try {
      const next = await api().answerPermission(response)
      applyRemoteState(next)
    } catch (error) {
      showErrorToast(error, "Could not answer permission request.")
      await loadStateSnapshot().catch(() => {})
    } finally {
      setPermissionBusy(false)
      promptRef?.focus()
    }
  }

  const answerQuestion = async (answers: string[][]) => {
    if (questionBusy()) return
    setQuestionBusy(true)
    try {
      const next = await api().answerQuestion(answers)
      applyRemoteState(next)
    } catch (error) {
      showErrorToast(error, "Could not answer question.")
      await loadStateSnapshot().catch(() => {})
    } finally {
      setQuestionBusy(false)
      promptRef?.focus()
    }
  }

  const cancelQuestion = async () => {
    if (questionBusy()) return
    setQuestionBusy(true)
    try {
      const next = await api().cancelQuestion()
      applyRemoteState(next)
    } catch (error) {
      showErrorToast(error, "Could not cancel question.")
      await loadStateSnapshot().catch(() => {})
    } finally {
      setQuestionBusy(false)
      promptRef?.focus()
    }
  }

  const loadModels = async () => {
    if (models().length > 0) return
    try {
      setModels(await api().models())
    } catch (error) {
      showErrorToast(error, "Could not load models.")
    }
  }

  const loadSkills = async () => {
    if (skills().length > 0) return
    try {
      setSkills(await api().skills())
    } catch (error) {
      showErrorToast(error, "Could not load skills.")
    }
  }

  const focusModelSearch = () => {
    window.setTimeout(() => modelSearchRef?.focus(), 0)
  }

  const focusPromptInput = () => {
    window.setTimeout(() => promptRef?.focus(), 0)
  }

  const requestPromptFocusAfterControlPopoverClose = () => {
    focusPromptAfterControlPopoverClose = true
  }

  const handleControlPopoverCloseAutoFocus = (event: Event) => {
    if (!focusPromptAfterControlPopoverClose) return
    event.preventDefault()
    focusPromptAfterControlPopoverClose = false
    focusPromptInput()
  }

  const handleModelOpenChange = (open: boolean) => {
    setModelOpen(open)
    if (open) {
      setModelQuery("")
      setModelActiveIndex(Math.max(filteredModels().findIndex((model) => model.active), 0))
      setComposerSuggestions([])
      setCompletionTrigger(null)
      void loadModels()
      focusModelSearch()
    }
  }

  const handleModelSearchKeyDown = (event: KeyboardEvent & { currentTarget: HTMLInputElement }) => {
    const list = filteredModels()
    if (event.key === "ArrowDown") {
      event.preventDefault()
      setModelActiveIndex((index) => (index + 1) % Math.max(list.length, 1))
      return
    }
    if (event.key === "ArrowUp") {
      event.preventDefault()
      setModelActiveIndex((index) => (index - 1 + Math.max(list.length, 1)) % Math.max(list.length, 1))
      return
    }
    if (event.key === "Enter") {
      event.preventDefault()
      const selected = list[modelActiveIndex()]
      if (selected) void selectModel(selected)
      return
    }
    if (event.key === "Escape") {
      event.preventDefault()
      requestPromptFocusAfterControlPopoverClose()
      setModelOpen(false)
    }
  }

  const handleNewProjectOpenChange = (open: boolean) => {
    setNewProjectOpen(open)
    setProjectPathError("")
    if (open) {
      setProjectPathInput("")
      queueMicrotask(() => projectPathInputRef?.focus())
    }
  }

  const handleProjectPickerOpenChange = (open: boolean) => {
    setProjectPickerOpen(open)
    setProjectPathError("")
    if (!open) setProjectPickerAddOpen(false)
  }

  const handleAgentOpenChange = (open: boolean) => {
    setAgentOpen(open)
    if (open) {
      setAgentActiveIndex(Math.max(AGENT_MODES.findIndex((agent) => sameToken(agent, state()?.status.agent || "Build")), 0))
      setComposerSuggestions([])
      setCompletionTrigger(null)
    }
  }

  const handleReasoningOpenChange = (open: boolean) => {
    setReasoningOpen(open)
    if (open) {
      setReasoningActiveIndex(
        Math.max(reasoningOptions().findIndex((effort) => sameToken(effort, reasoningLabel())), 0)
      )
      setComposerSuggestions([])
      setCompletionTrigger(null)
    }
  }

  const selectModel = async (model: RemoteModel) => {
    try {
      await api().selectModel(model.provider_id, model.id)
      setModelOpen(false)
      setModels([])
      await loadStateSnapshot()
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not select model.")
    }
  }

  const selectAgentMode = async (agent: string) => {
    try {
      const next = await api().setAgent(agent)
      applyRemoteState(next)
      setAgentOpen(false)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not switch agent.")
    }
  }

  const selectReasoningEffort = async (effort: string) => {
    try {
      const next = await api().setReasoning(effort === "off" ? null : effort)
      applyRemoteState(next)
      setReasoningOpen(false)
      promptRef?.focus()
    } catch (error) {
      showErrorToast(error, "Could not set reasoning effort.")
    }
  }

  const handleAgentMenuKeyDown = (event: KeyboardEvent) => {
    handleChoiceMenuKeyDown(
      event,
      agentOpen(),
      setAgentOpen,
      AGENT_MODES,
      agentActiveIndex(),
      setAgentActiveIndex,
      selectAgentMode,
      requestPromptFocusAfterControlPopoverClose
    )
  }

  const handleReasoningMenuKeyDown = (event: KeyboardEvent) => {
    handleChoiceMenuKeyDown(
      event,
      reasoningOpen(),
      setReasoningOpen,
      reasoningOptions(),
      reasoningActiveIndex(),
      setReasoningActiveIndex,
      selectReasoningEffort,
      requestPromptFocusAfterControlPopoverClose
    )
  }

  const openServerManager = () => {
    setServersOpen(false)
    setServersManageOpen(true)
    setServerAddOpen(false)
    setServerSearch("")
  }

  const handleServersOpenChange = (open: boolean) => {
    setServersOpen(open)
    if (open) {
      setServerPanelTab("servers")
      void loadSkills()
    }
  }

  const selectServerPanelTab = (tab: ServerPanelTab) => {
    setServerPanelTab(tab)
    if (tab === "skills") void loadSkills()
  }

  const showAddServer = () => {
    setServerAddOpen(true)
    setServerAddress("")
    setServerName("")
    setServerUsername("")
    setServerPassword("")
    queueMicrotask(() => serverAddressRef?.focus())
  }

  const saveServer = (event: SubmitEvent) => {
    event.preventDefault()
    const address = normalizeServerAddress(serverAddress())
    if (!address) return

    const nextServer: SavedServer = {
      id: cuid(),
      address,
      name: serverName().trim() || address.replace(/^https?:\/\//, ""),
      username: serverUsername().trim(),
      password: serverPassword(),
    }
    const next = [
      nextServer,
      ...savedServers().filter(
        (server) => normalizeServerAddress(server.address) !== normalizeServerAddress(address)
      ),
    ]
    setSavedServers(next)
    saveSavedServers(next)
    setServerAddOpen(false)
  }

  const openServer = (server: SavedServer) => {
    const address = normalizeServerAddress(server.address)
    if (!address || isActiveServer(address, activeServerUrl())) return
    window.location.href = address
  }

  const toggleProject = (key: string) => {
    setProjectOpen((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const refreshCompletion = () => {
    const trigger = detectCompletionTrigger(prompt(), promptRef?.selectionStart ?? prompt().length)
    setCompletionTrigger(trigger)
    setCompletionRevision((value) => value + 1)
  }

  const applyComposerSuggestion = (suggestion: RemoteSuggestion) => {
    const trigger = completionTrigger()
    if (!trigger) return

    const text = prompt()
    const [start, end] = trigger.range
    const replacement =
      suggestion.kind === "command"
        ? `/${suggestion.replacement} `
        : suggestion.kind === "agent"
          ? `@${suggestion.replacement} `
          : `${quoteCompletionPath(suggestion.replacement)} `
    const next = `${text.slice(0, start)}${replacement}${text.slice(end)}`
    const cursor = start + replacement.length
    resetPromptHistoryNavigation()
    setPrompt(next)
    setComposerSuggestions([])
    setCompletionTrigger(null)
    queueMicrotask(() => {
      if (!promptRef) return
      promptRef.focus()
      promptRef.setSelectionRange(cursor, cursor)
      resizePrompt()
    })
  }

  const chooseComposerSuggestion = (suggestion: RemoteSuggestion) => {
    if (suggestion.kind === "command") {
      void submitPromptText(`/${suggestion.replacement || suggestion.name}`, false)
      return
    }

    applyComposerSuggestion(suggestion)
  }

  const handlePromptKeyDown = (event: KeyboardEvent & { currentTarget: HTMLTextAreaElement }) => {
    if (composerSuggestions().length > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault()
        setComposerSuggestionIndex((index) => (index + 1) % composerSuggestions().length)
        return
      }
      if (event.key === "ArrowUp") {
        event.preventDefault()
        setComposerSuggestionIndex((index) =>
          (index - 1 + composerSuggestions().length) % composerSuggestions().length
        )
        return
      }
      if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) {
        event.preventDefault()
        const selected = composerSuggestions()[composerSuggestionIndex()]
        if (selected) chooseComposerSuggestion(selected)
        return
      }
      if (event.key === "Escape") {
        event.preventDefault()
        setComposerSuggestions([])
        setCompletionTrigger(null)
        return
      }
    }

    if (removePromptImageTagAtCursor(event)) return

    if (event.key === "ArrowUp" && navigatePromptHistory("up", event)) return
    if (event.key === "ArrowDown" && navigatePromptHistory("down", event)) return

    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  const modelLabel = createMemo(() => {
    const status = state()?.status
    if (!status) return "Model"
    return `${status.provider}/${status.model}`
  })

  const themeStyle = createMemo(
    () =>
      ({
        "--brand-primary": state()?.status.theme?.primary ?? "#6c8ed8",
        "--brand-dim": state()?.status.theme?.primary_dim ?? "#4a639f",
      }) as JSX.CSSProperties
  )

  const resizePrompt = () => {
    const el = promptRef
    if (!el) return
    el.style.height = "auto"
    el.style.height = `${Math.min(el.scrollHeight, 224)}px`
  }

  return (
    <div
      class={cx(
        "grid h-dvh overflow-hidden bg-[var(--bg)] min-[901px]:grid-cols-[clamp(16.5rem,19vw,20rem)_minmax(0,1fr)] max-[900px]:grid-cols-1"
      )}
      style={themeStyle()}
    >
      <Show when={pairRequired()}>
        <div class="fixed inset-0 z-[100] grid items-start justify-items-center bg-black/60 px-4 pt-[min(14vh,7rem)] pb-4">
          <form class={cx(PANEL_BASE, "w-[min(100%,28rem)] p-5")} onSubmit={pair}>
            <h1 class="m-0 text-[1.1rem] font-semibold text-[var(--text)]">Pair device</h1>
            <p class="my-3 text-[var(--muted)] leading-relaxed">Enter the code printed by crabcode serve.</p>
            <div class="grid grid-cols-[1fr_auto] gap-2 max-[560px]:grid-cols-1">
              <input
                class={cx(INPUT_BASE, "h-10 font-mono")}
                value={pairCode()}
                onInput={(event) => setPairCode(event.currentTarget.value)}
                autocomplete="one-time-code"
                inputmode="numeric"
                placeholder="482-119"
              />
              <button class="h-10 rounded-lg bg-[#e5e2dc] px-4 font-bold text-[#171717]" type="submit">
                Connect
              </button>
            </div>
            <div class="mt-3 min-h-4 text-[0.82rem] text-[var(--red)]">{pairError()}</div>
          </form>
        </div>
      </Show>

      <aside
        class={cx(
          "flex h-dvh min-w-0 flex-col overflow-hidden border-r border-[var(--line)] bg-[var(--panel)] max-[900px]:fixed max-[900px]:inset-y-0 max-[900px]:left-0 max-[900px]:z-[80] max-[900px]:w-[min(25rem,88vw)] max-[900px]:transition-transform max-[900px]:duration-150",
          sidebarOpen() ? "max-[900px]:translate-x-0" : "max-[900px]:-translate-x-[101%]"
        )}
      >
        <button
          class="group mx-6 mt-4 mb-5 flex items-center gap-2 rounded-lg bg-[#1d1d1d] px-3 py-2 text-[var(--muted)] transition hover:bg-[#282828] hover:text-[var(--text)]"
          type="button"
          onClick={openCommandPalette}
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
            open={newProjectOpen()}
            onOpenChange={handleNewProjectOpenChange}
            placement="bottom-end"
            gutter={8}
          >
            <PopoverTrigger as="button" class={cx(ICON_BUTTON, "h-7 w-7")} type="button" title="Open folder">
              <IconPlus class="h-4 w-4" />
            </PopoverTrigger>
            <PopoverContent class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] w-[min(24rem,calc(100vw-1.4rem))] p-3")}>
              <form class="grid grid-cols-[minmax(0,1fr)_auto] gap-2" onSubmit={submitProjectPath}>
                <input
                  class={cx(INPUT_BASE, "h-10 font-mono text-[0.78rem]")}
                  ref={projectPathInputRef}
                  value={projectPathInput()}
                  onInput={(event) => setProjectPathInput(event.currentTarget.value)}
                  placeholder="/Users/carlo/Desktop/Projects/app"
                />
                <button
                  class="h-10 rounded-lg bg-[#e5e2dc] px-3 text-[0.82rem] font-bold text-[#171717]"
                  type="submit"
                  disabled={!projectPathInput().trim()}
                >
                  Open
                </button>
              </form>
              <Show when={projectPathError()}>
                <div class="mt-2 text-[0.76rem] leading-snug text-[var(--red)]">{projectPathError()}</div>
              </Show>
            </PopoverContent>
          </Popover>
        </div>

        <ProjectList
          projects={projects}
          openProjects={projectOpen}
          activeProjectPath={projectPath}
          token={token}
          currentSessionId={() => state()?.current_session_id}
          onToggleProject={toggleProject}
          onNewSession={startNewSession}
          onSwitchSession={switchSession}
          onArchiveSession={archiveSession}
          onArchiveProject={archiveProject}
        />
      </aside>
      <button
        class={cx(
          "fixed inset-0 z-[70] hidden bg-black/45 max-[900px]:block",
          sidebarOpen() ? "max-[900px]:block" : "max-[900px]:hidden"
        )}
        type="button"
        onClick={() => setSidebarOpen(false)}
      />

      <main class="relative flex h-dvh min-h-0 min-w-0 flex-col overflow-hidden bg-[#171717]">
        <header class="flex h-[4.8rem] flex-none items-center justify-between gap-4 border-b border-[var(--line)] bg-[#181818] px-8 max-[900px]:px-4">
          <button
            class="hidden h-[2.2rem] w-[2.2rem] place-items-center rounded-lg border border-[var(--line)] text-[var(--muted)] max-[900px]:inline-grid"
            type="button"
            onClick={() => setSidebarOpen(true)}
            aria-label="Open projects"
          >
            <IconSidebar class="h-[1.1rem] w-[1.1rem]" />
          </button>
          <Popover
            open={projectPickerOpen()}
            onOpenChange={handleProjectPickerOpenChange}
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
                  {projectName()}
                </span>
                <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.72rem] text-[var(--faint)] max-[560px]:hidden">
                  {projectPath()}
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
                  onClick={() => {
                    setProjectPickerAddOpen((open) => !open)
                    setProjectPathError("")
                    setProjectPathInput("")
                    queueMicrotask(() => projectPathInputRef?.focus())
                  }}
                >
                  <IconPlus class="h-[0.95rem] w-[0.95rem]" />
                </button>
              </div>
              <Show when={projectPickerAddOpen()}>
                <form class="grid grid-cols-[minmax(0,1fr)_auto] gap-2 border-b border-[var(--line)] p-3" onSubmit={submitProjectPath}>
                  <input
                    class={cx(INPUT_BASE, "h-10 font-mono text-[0.78rem]")}
                    ref={projectPathInputRef}
                    value={projectPathInput()}
                    onInput={(event) => setProjectPathInput(event.currentTarget.value)}
                    placeholder="/Users/carlo/Desktop/Projects/app"
                  />
                  <button
                    class="h-10 rounded-lg bg-[#e5e2dc] px-3 text-[0.82rem] font-bold text-[#171717]"
                    type="submit"
                    disabled={!projectPathInput().trim()}
                  >
                    Open
                  </button>
                </form>
              </Show>
              <div class="min-h-0 flex-1 overflow-y-auto p-2">
                <For each={projects()}>
                  {(project) => (
                    <button
                      class={cx(
                        "grid w-full min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-lg p-2 text-left text-[var(--text)] hover:bg-white/[0.055]",
                        project.path === projectPath() && "bg-white/[0.055]"
                      )}
                      type="button"
                      onClick={() => selectWorkspace(project.path)}
                    >
                      <ProjectFavicon
                        cwd={project.path}
                        label={project.name}
                        token={token()}
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
              <Show when={projectPathError()}>
                <div class="border-t border-[var(--line)] px-3 py-2 text-[0.76rem] leading-snug text-[var(--red)]">
                  {projectPathError()}
                </div>
              </Show>
            </PopoverContent>
          </Popover>
          <div class="ml-auto flex items-center gap-2">
            <Show when={!isEmptyChat()}>
              <button
                class="inline-flex h-[2.2rem] items-center gap-2 rounded-lg border border-[var(--line-strong)] bg-[#222222] px-3 text-[0.86rem] font-semibold text-[#d7d5d0] transition hover:border-[rgba(255,255,255,0.18)] hover:bg-[#2b2b2b] hover:text-[var(--text)] max-[560px]:aspect-square max-[560px]:w-[2.2rem] max-[560px]:justify-center max-[560px]:p-0"
                type="button"
                onClick={() => startNewSession()}
              >
                <IconPlus class="h-4 w-4" />
                <span class="max-[560px]:hidden">New chat</span>
              </button>
            </Show>
            <Popover open={serversOpen()} onOpenChange={handleServersOpenChange} placement="bottom-end" gutter={8}>
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
                      serverPanelTab() === "servers" && "border-b-2 border-[var(--text)] text-[var(--text)]"
                    )}
                    type="button"
                    onClick={() => selectServerPanelTab("servers")}
                  >
                    Servers <CountBadge count={servers().length} />
                  </button>
                  <button
                    class={cx(
                      "inline-flex items-center gap-1.5 py-1 text-[0.9rem] text-[var(--muted)]",
                      serverPanelTab() === "skills" && "border-b-2 border-[var(--text)] text-[var(--text)]"
                    )}
                    type="button"
                    onClick={() => selectServerPanelTab("skills")}
                  >
                    Skills <CountBadge count={skills().length} />
                  </button>
                  <button
                    class={cx("py-1 text-[0.9rem] text-[var(--muted)]", serverPanelTab() === "mcp" && "text-[var(--text)]")}
                    type="button"
                    onClick={() => selectServerPanelTab("mcp")}
                    disabled
                  >
                    MCP
                  </button>
                  <button
                    class={cx("py-1 text-[0.9rem] text-[var(--muted)]", serverPanelTab() === "lsp" && "text-[var(--text)]")}
                    type="button"
                    onClick={() => selectServerPanelTab("lsp")}
                    disabled
                  >
                    LSP
                  </button>
                  <button
                    class={cx("py-1 text-[0.9rem] text-[var(--muted)]", serverPanelTab() === "plugins" && "text-[var(--text)]")}
                    type="button"
                    onClick={() => selectServerPanelTab("plugins")}
                    disabled
                  >
                    Plugins
                  </button>
                </div>
                <Show
                  when={serverPanelTab() === "skills"}
                  fallback={
                    <>
                      <div class="grid gap-1 py-3">
                        <For each={servers().slice(0, 3)}>
                          {(server) => (
                            <button
                              class="grid min-h-10 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-2 rounded-lg px-2 text-left text-[var(--muted)] hover:bg-white/[0.04] hover:text-[var(--text)]"
                              type="button"
                              onClick={() => openServer(server)}
                            >
                              <span class="h-2 w-2 rounded-full bg-[#53b842]" />
                              <span>{server.name}</span>
                              <span class="text-[0.78rem] text-[var(--faint)]">v{state()?.status.version}</span>
                              <Show when={isActiveServer(server.address, activeServerUrl())}>
                                <IconCheck class="h-4 w-4 text-[var(--muted)]" />
                              </Show>
                            </button>
                          )}
                        </For>
                      </div>
                      <button
                        class="inline-flex h-9 items-center justify-center rounded-lg border border-[var(--line-strong)] px-3 text-[0.84rem] font-medium text-[var(--text)] hover:bg-white/[0.045]"
                        type="button"
                        onClick={openServerManager}
                      >
                        Manage servers
                      </button>
                    </>
                  }
                >
                  <div class="grid max-h-76 gap-1 overflow-auto py-3 pb-1">
                    <Show when={skills().length > 0} fallback={<div class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">No skills loaded.</div>}>
                      <For each={skills()}>
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
          </div>
        </header>

        <div class="relative min-h-0 flex-1">
          <div
            ref={setThreadScrollEl}
            class={cx(
              "h-full min-h-0 overflow-y-auto overflow-x-hidden px-4 pt-5",
              isEmptyChat() ? "overflow-hidden pb-[clamp(8rem,22vh,12rem)]" : "pb-52"
            )}
          >
            <div ref={setThreadContentEl} class={cx("mx-auto w-[min(100%,64rem)]", isEmptyChat() && "grid h-full")}>
            <Show
              when={visibleMessages().length > 0}
              fallback={
                <EmptyThread
                  projectName={projectName()}
                  mascotFrame={MASCOT_FRAMES[mascotFrame()] ?? ""}
                />
              }
            >
              <Index each={threadItems()}>
                {(item) => (
                  <ThreadItemView
                    item={item}
                    status={() => state()?.status ?? null}
                    token={token}
                    onPreviewImage={openImagePreview}
                  />
                )}
              </Index>
            </Show>
            </div>
          </div>
          <FadedEdgeEffect direction="top" hidden={threadScroll.isAtTop()} size="3rem" color="#171717" />
          <FadedEdgeEffect direction="bottom" hidden={threadScroll.isAtBottom()} size="7rem" color="#171717" />
        </div>

        <div class="pointer-events-none absolute right-0 bottom-0 left-0 z-30 grid flex-none gap-3 px-4 pb-[max(1rem,env(safe-area-inset-bottom))] max-[900px]:px-3">
          <Show when={pendingPermission()}>
            {(permission) => (
              <PermissionRequestPanel
                permission={permission()}
                busy={permissionBusy()}
                onAnswer={answerPermission}
              />
            )}
          </Show>
          <Show when={!pendingPermission() ? pendingQuestion() : null}>
            {(question) => (
              <QuestionRequestPanel
                prompt={question()}
                busy={questionBusy()}
                onSubmit={answerQuestion}
                onCancel={cancelQuestion}
              />
            )}
          </Show>
          <form
            class="pointer-events-auto relative mx-auto w-[min(100%,67rem)] overflow-visible rounded-[18px] border border-[var(--line-strong)] bg-[var(--composer)] shadow-[0_0.5rem_2.5rem_var(--shadow)]"
            onSubmit={submitPrompt}
            onDragOver={(event) => event.preventDefault()}
            onDrop={handleComposerDrop}
          >
            <input
              ref={imageInputRef}
              class="hidden"
              type="file"
              accept={IMAGE_FILE_TYPES.join(",")}
              multiple
              onChange={(event) => {
                const files = Array.from(event.currentTarget.files ?? [])
                event.currentTarget.value = ""
                void addImageFiles(files)
              }}
            />
            <Show when={composerAttachments().length > 0}>
              <div class="max-h-40 overflow-y-auto px-3 pt-3">
                <Attachments variant="grid" class="grid-cols-[repeat(auto-fill,minmax(8rem,1fr))]">
                  <For each={composerAttachmentData()}>
                    {(attachment) => (
                      <Attachment
                        data={attachment}
                        onRemove={() => removeComposerAttachment(attachment.id)}
                        class="cursor-zoom-in transition hover:border-[rgba(255,255,255,0.16)] hover:bg-[#242424] focus-visible:border-[rgba(157,177,239,0.55)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[rgba(157,177,239,0.18)]"
                        role="button"
                        tabIndex={0}
                        onClick={() => openImagePreview(attachment)}
                        onKeyDown={(event) => handleImagePreviewKeyDown(event, () => openImagePreview(attachment))}
                      >
                        <AttachmentPreview />
                        <div class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-1 px-2 py-1.5">
                          <AttachmentInfo />
                          <AttachmentRemove class="opacity-70 group-hover:opacity-100" />
                        </div>
                      </Attachment>
                    )}
                  </For>
                </Attachments>
              </div>
            </Show>
            <div class="relative">
              <div
                ref={promptOverlayRef}
                class={cx(
                  "pointer-events-none absolute inset-0 max-h-56 min-h-[4.9rem] overflow-hidden whitespace-pre-wrap break-words border-0 bg-transparent text-[var(--text)]",
                  COMPOSER_TEXT_CLASS
                )}
                aria-hidden="true"
              >
                <For each={promptTextParts(prompt(), composerAttachments().length)}>
                  {(part) => (
                    <span
                      class={promptTextPartClass(part)}
                      style={promptTextPartStyle(part)}
                    >
                      {part.text}
                    </span>
                  )}
                </For>
                <span aria-hidden="true">&#8203;</span>
              </div>
              <textarea
                ref={promptRef}
                class={cx(
                  "relative z-10 block max-h-56 min-h-[4.9rem] w-full resize-none border-0 bg-transparent text-transparent caret-[var(--text)] outline-none placeholder:text-[#54524e] selection:bg-[rgba(126,157,234,0.28)]",
                  COMPOSER_TEXT_CLASS
                )}
                value={prompt()}
                onInput={handlePromptInput}
                onKeyDown={handlePromptKeyDown}
                onKeyUp={refreshCompletion}
                onClick={refreshCompletion}
                onScroll={handlePromptScroll}
                onPaste={handlePromptPaste}
                placeholder="Ask for follow-up changes or attach images"
                rows={1}
              />
            </div>
            <Show when={composerSuggestions().length > 0}>
              <div
                ref={composerSuggestionsRef}
                class="absolute right-4 bottom-[calc(100%+0.6rem)] left-4 max-h-[min(22rem,42vh)] overflow-auto rounded-[14px] border border-[var(--line-strong)] bg-[#171717] p-2 shadow-[0_1rem_2.4rem_var(--shadow)]"
                role="listbox"
              >
                <For each={composerSuggestions()}>
                  {(suggestion, index) => (
                    <button
                      class={cx(
                        "grid min-h-[3.05rem] w-full grid-cols-[1.7rem_minmax(0,1fr)] items-center gap-3 rounded-[9px] px-2 py-1.5 text-left text-[var(--text)] hover:bg-white/[0.07]",
                        index() === composerSuggestionIndex() && "bg-white/[0.07]"
                      )}
                      type="button"
                      role="option"
                      aria-selected={index() === composerSuggestionIndex()}
                      data-composer-suggestion-index={index()}
                      onMouseEnter={() => setComposerSuggestionIndex(index())}
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => chooseComposerSuggestion(suggestion)}
                    >
                      <SuggestionIcon suggestion={suggestion} />
                      <span class="flex min-w-0 flex-col gap-0.5">
                        <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.92rem] font-semibold text-[var(--text)]">
                          <span class="text-[var(--muted)]">{suggestionPrefix(suggestion)}</span>
                          {suggestion.name}
                        </span>
                        <Show when={suggestion.description}>
                          {(description) => (
                            <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">
                              {description()}
                            </span>
                          )}
                        </Show>
                      </span>
                    </button>
                  )}
                </For>
              </div>
            </Show>
            <div class="flex items-center justify-between gap-4 px-4 pt-2 pb-2.5">
              <div class="flex min-w-0 flex-1 items-center gap-3 max-[560px]:gap-2">
                <button
                  class={cx(ICON_BUTTON, "h-[1.95rem] w-[1.95rem] shrink-0 border border-[var(--line)] bg-[#202020]")}
                  type="button"
                  aria-label="Attach image"
                  title="Attach image"
                  onClick={() => imageInputRef?.click()}
                >
                  <IconPaperclip class="h-4 w-4" />
                </button>
                <div class="min-w-0 flex-[0_1_auto] max-[560px]:flex-1">
                  <Popover open={modelOpen()} onOpenChange={handleModelOpenChange} placement="top-start" gutter={10}>
                    <PopoverTrigger
                      as="button"
                      class="inline-flex h-[1.95rem] max-w-[min(38vw,18rem)] min-w-0 items-center gap-2 rounded-[7px] border border-[var(--line)] bg-[#202020] px-2.5 text-[0.86rem] text-[var(--muted)] transition hover:bg-[#252525] hover:text-[var(--text)] focus-visible:bg-[#252525] focus-visible:text-[var(--text)] max-[900px]:max-w-[44vw] max-[560px]:max-w-full"
                      type="button"
                    >
                      <IconBrainGlyph class="h-[1.05rem] w-[1.05rem] shrink-0" />
                      <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{modelLabel()}</span>
                      <IconCaretDown class="h-3 w-3 shrink-0 text-[var(--faint)]" />
                    </PopoverTrigger>
                    <PopoverContent
                      class={cx(
                        PANEL_BASE,
                        POPOVER_ANIMATION,
                        "z-[90] grid max-h-[min(30rem,62vh)] w-[min(26rem,calc(100vw-1.4rem))] grid-rows-[auto_1fr] overflow-hidden"
                      )}
                      onCloseAutoFocus={handleControlPopoverCloseAutoFocus}
                      onEscapeKeyDown={requestPromptFocusAfterControlPopoverClose}
                    >
                      <input
                        ref={modelSearchRef}
                        class="h-[2.55rem] w-full border-0 border-b border-[var(--line)] bg-transparent px-3 text-[var(--text)] outline-none"
                        placeholder="Search models"
                        value={modelQuery()}
                        onInput={(event) => setModelQuery(event.currentTarget.value)}
                        onKeyDown={handleModelSearchKeyDown}
                        role="combobox"
                        aria-expanded={modelOpen()}
                        aria-controls="model-listbox"
                      />
                      <div id="model-listbox" class="min-h-0 overflow-y-auto overscroll-contain p-2" role="listbox">
                        <ModelList
                          models={filteredModels()}
                          activeIndex={modelActiveIndex()}
                          onActiveIndex={setModelActiveIndex}
                          onSelect={selectModel}
                        />
                      </div>
                    </PopoverContent>
                  </Popover>
                </div>
                <Popover open={agentOpen()} onOpenChange={handleAgentOpenChange} placement="top-start" gutter={8}>
                  <PopoverTrigger
                    as="button"
                    class="inline-flex h-[1.95rem] shrink-0 items-center justify-center gap-1.5 rounded-[7px] border border-[var(--line)] bg-[#202020] px-3 text-[0.78rem] font-semibold text-[var(--muted)] hover:bg-[#252525] hover:text-[var(--text)] max-[560px]:px-2"
                    type="button"
                    onKeyDown={handleAgentMenuKeyDown}
                  >
                    <span>{state()?.status.agent || "Build"}</span>
                    <IconCaretDown class="h-3 w-3 text-[var(--faint)]" />
                  </PopoverTrigger>
                  <PopoverContent
                    class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] min-w-36 overflow-hidden p-1")}
                    tabIndex={-1}
                    onCloseAutoFocus={handleControlPopoverCloseAutoFocus}
                    onEscapeKeyDown={requestPromptFocusAfterControlPopoverClose}
                    onKeyDown={handleAgentMenuKeyDown}
                  >
                    <For each={AGENT_MODES}>
                      {(agent, index) => (
                        <button
                          class={cx(
                            MENU_ROW,
                            (sameToken(agent, state()?.status.agent || "Build") || index() === agentActiveIndex()) &&
                              MENU_ROW_ACTIVE
                          )}
                          type="button"
                          onClick={() => selectAgentMode(agent)}
                          onMouseEnter={() => setAgentActiveIndex(index())}
                        >
                          <span>{agent}</span>
                          <Show when={sameToken(agent, state()?.status.agent || "Build")}>
                            <IconCheck class="h-3.5 w-3.5 text-[var(--muted)]" />
                          </Show>
                        </button>
                      )}
                    </For>
                  </PopoverContent>
                </Popover>
                <Show when={reasoningOptions().length > 0}>
                  <Popover open={reasoningOpen()} onOpenChange={handleReasoningOpenChange} placement="top-start" gutter={8}>
                    <PopoverTrigger
                      as="button"
                      class="inline-flex h-[1.95rem] min-w-[4.6rem] shrink-0 items-center justify-center gap-1.5 rounded-[7px] border border-[var(--line)] bg-[#202020] px-3 text-[0.78rem] font-semibold text-[var(--muted)] hover:bg-[#252525] hover:text-[var(--text)] max-[560px]:min-w-[3.9rem] max-[560px]:px-2"
                      type="button"
                      onKeyDown={handleReasoningMenuKeyDown}
                    >
                      <span>{reasoningLabel()}</span>
                      <IconCaretDown class="h-3 w-3 text-[var(--faint)]" />
                    </PopoverTrigger>
                    <PopoverContent
                      class={cx(PANEL_BASE, POPOVER_ANIMATION, "z-[90] min-w-36 overflow-hidden p-1")}
                      tabIndex={-1}
                      onCloseAutoFocus={handleControlPopoverCloseAutoFocus}
                      onEscapeKeyDown={requestPromptFocusAfterControlPopoverClose}
                      onKeyDown={handleReasoningMenuKeyDown}
                    >
                      <For each={reasoningOptions()}>
                        {(effort, index) => (
                          <button
                            class={cx(
                              MENU_ROW,
                              (sameToken(effort, reasoningLabel()) || index() === reasoningActiveIndex()) &&
                                MENU_ROW_ACTIVE
                            )}
                            type="button"
                            onClick={() => selectReasoningEffort(effort)}
                            onMouseEnter={() => setReasoningActiveIndex(index())}
                          >
                            <span>{effort}</span>
                            <Show when={sameToken(effort, reasoningLabel())}>
                              <IconCheck class="h-3.5 w-3.5 text-[var(--muted)]" />
                            </Show>
                          </button>
                        )}
                      </For>
                    </PopoverContent>
                  </Popover>
                </Show>
              </div>
              <div class="flex min-w-0 items-center gap-3">
                <button
                  class={cx(
                    "grid h-11 w-11 place-items-center rounded-full transition shadow-[inset_0_0_0_1px_rgba(255,255,255,0.08)]",
                    state()?.is_streaming
                      ? "bg-[#3c2528] text-[#d4929a] hover:bg-[#482b2f]"
                      : "bg-[var(--brand-primary)] text-[#111318] hover:bg-[#7d9dea]"
                  )}
                  type="submit"
                  aria-label={state()?.is_streaming ? "Stop" : "Send"}
                >
                  <Show
                    when={state()?.is_streaming}
                    fallback={<IconArrowUp class="h-[1.15rem] w-[1.15rem]" />}
                  >
                    <span class="h-3 w-3 rounded-[2px] bg-current" />
                  </Show>
                </button>
              </div>
            </div>
          </form>
        </div>
      </main>

      <Show when={commandRendered()}>
        <div
          class={cx(
            "fixed inset-0 z-[100] grid place-items-start justify-items-center bg-black/60 px-4 pt-[min(14vh,7rem)] pb-4",
            commandClosing() ? "animate-fadeOut" : "animate-fadeIn"
          )}
          onMouseDown={(event) => event.currentTarget === event.target && closeCommandPalette()}
        >
          <div
            class={cx(
              PANEL_BASE,
              "grid max-h-[min(34rem,calc(100dvh-min(14vh,7rem)-1rem))] w-[min(100%,43rem)] grid-rows-[auto_minmax(0,1fr)] overflow-hidden origin-top will-change-transform",
              commandClosing() ? "animate-flyUpAndScaleExit" : "animate-flyUpAndScale"
            )}
          >
            <Command class="grid h-full min-h-0 grid-rows-[auto_minmax(0,1fr)]" shouldFilter={false} loop>
              <div class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 border-b border-[var(--line)] px-3" cmdk-input-wrapper="">
                <IconSearch class="h-4 w-4 text-[var(--faint)]" />
                <CommandInput
                  ref={commandInputRef}
                  class="h-[2.65rem] min-w-0 border-0 bg-transparent text-[var(--text)] outline-none"
                  placeholder="Search projects and sessions"
                  value={commandQuery()}
                  onValueChange={setCommandQuery}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") closeCommandPalette()
                  }}
                />
              </div>
              <CommandList class="min-h-0 overflow-y-auto overscroll-contain p-2">
                <CommandEmpty class="px-2 py-4 text-[0.84rem] text-[var(--faint)]">No projects or sessions found.</CommandEmpty>
                <Show when={!isEmptyChat()}>
                  <CommandGroup heading="Actions" forceMount>
                    <CommandItem
                      class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                      value="new-chat"
                      onSelect={() => startNewSession()}
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
                  <For each={projectCommandResults()}>
                    {(project) => (
                      <CommandItem
                        class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                        value={`project-${project.path}`}
                        keywords={[project.name, project.path]}
                        onSelect={() => startNewSession(project.path)}
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
                  <For each={commandResults()}>
                    {(session) => (
                      <CommandItem
                        class="flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] aria-selected:bg-white/[0.055]"
                        value={session.id}
                        keywords={[session.title, session.workspace]}
                        onSelect={() => switchSession(session.id)}
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

      <Show when={serversManageOpen()}>
        <div
          class="fixed inset-0 z-[120] grid place-items-center bg-black/55 p-4 animate-fadeIn"
          onMouseDown={(event) => event.currentTarget === event.target && setServersManageOpen(false)}
        >
          <section class="flex max-h-[min(42rem,calc(100dvh-2rem))] w-[min(100%,48rem)] flex-col overflow-hidden rounded-[14px] border border-[var(--line-strong)] bg-[#1a1a1a] shadow-[0_1.2rem_4rem_rgba(0,0,0,0.42)] animate-flyUpAndScale">
            <div class="flex min-h-[4.2rem] flex-none items-center justify-between gap-4 px-6">
              <Show
                when={serverAddOpen()}
                fallback={<h2 class="m-0 text-[1.2rem] font-semibold text-[var(--text)]">Servers</h2>}
              >
                <button class="inline-flex items-center gap-3 text-[var(--muted)] hover:text-[var(--text)]" type="button" onClick={() => setServerAddOpen(false)}>
                  <IconArrowLeft class="h-[1.15rem] w-[1.15rem]" />
                  <span>Add server</span>
                </button>
              </Show>
              <button
                class="inline-flex h-8 w-8 items-center justify-center rounded-lg text-[var(--muted)] hover:text-[var(--text)]"
                type="button"
                onClick={() => setServersManageOpen(false)}
              >
                <IconX class="h-5 w-5" />
              </button>
            </div>

            <Show
              when={serverAddOpen()}
              fallback={
                <>
                  <div class="mx-6 mb-4 grid min-w-0 flex-none grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-[9px] bg-[#181818] px-3">
                    <IconSearch class="h-4 w-4 text-[var(--muted)]" />
                    <input
                      class="h-11 min-w-0 border-0 bg-transparent text-[var(--text)] outline-none"
                      value={serverSearch()}
                      onInput={(event) => setServerSearch(event.currentTarget.value)}
                      placeholder="Search servers"
                    />
                  </div>
                  <div class="grid min-h-0 flex-1 gap-2 overflow-y-auto px-6 pb-4">
                    <For each={filteredServers()}>
                      {(server) => (
                        <button
                          class="grid min-h-[4.6rem] min-w-0 grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-3 rounded-[9px] bg-[#1f1f1f] px-4 text-left text-[var(--text)] hover:bg-[#242424]"
                          type="button"
                          onClick={() => openServer(server)}
                        >
                          <span class="h-2 w-2 rounded-full bg-[#53b842]" />
                          <span class="flex min-w-0 flex-col gap-1">
                            <span class="flex min-w-0 items-baseline gap-2 overflow-hidden text-ellipsis whitespace-nowrap font-semibold">
                              {server.name}
                              <span class="text-[0.78rem] text-[var(--faint)]">v{state()?.status.version}</span>
                            </span>
                            <span class="overflow-hidden text-ellipsis whitespace-nowrap text-[0.82rem] text-[var(--faint)]">
                              {server.username || "no username"}
                            </span>
                          </span>
                          <Show when={isActiveServer(server.address, activeServerUrl())}>
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
                    onClick={showAddServer}
                  >
                    <IconPlus class="h-4 w-4" />
                    <span>Add server</span>
                  </button>
                </>
              }
            >
              <form class="mx-6 mb-6 grid min-h-0 flex-1 gap-4 overflow-y-auto rounded-[10px] bg-[#181818] p-5" onSubmit={saveServer}>
                <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                  <span>Server address</span>
                  <input
                    class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                    ref={serverAddressRef}
                    value={serverAddress()}
                    onInput={(event) => setServerAddress(event.currentTarget.value)}
                    placeholder="http://localhost:4096"
                  />
                </label>
                <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                  <span>Server name (optional)</span>
                  <input
                    class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                    value={serverName()}
                    onInput={(event) => setServerName(event.currentTarget.value)}
                    placeholder="Localhost"
                  />
                </label>
                <div class="grid grid-cols-2 gap-4 max-[560px]:grid-cols-1">
                  <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                    <span>Username (optional)</span>
                    <input
                      class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                      value={serverUsername()}
                      onInput={(event) => setServerUsername(event.currentTarget.value)}
                      placeholder="opencode"
                    />
                  </label>
                  <label class="flex min-w-0 flex-col gap-2 text-[0.82rem] font-semibold text-[var(--muted)]">
                    <span>Password (optional)</span>
                    <input
                      class="h-[2.65rem] min-w-0 rounded-lg border border-[var(--line-strong)] bg-[#131313] px-3 text-[0.9rem] font-medium text-[var(--text)] outline-none focus:border-[rgba(108,142,216,0.75)] focus:shadow-[0_0_0_2px_rgba(108,142,216,0.18)]"
                      type="password"
                      value={serverPassword()}
                      onInput={(event) => setServerPassword(event.currentTarget.value)}
                      placeholder="password"
                    />
                  </label>
                </div>
                <button
                  class="h-[2.55rem] w-fit rounded-lg bg-[#e5e2dc] px-4 font-bold text-[#171717]"
                  type="submit"
                  disabled={!serverAddress().trim()}
                >
                  Add server
                </button>
              </form>
            </Show>
          </section>
        </div>
      </Show>
      <ImagePreviewDialog image={imagePreview} onClose={() => setImagePreview(null)} />
    </div>
  )
}

function projectsFromState(state: RemoteState | null | undefined): ProjectGroup[] {
  const map = new Map<string, ProjectGroup>()
  const currentPath = state?.status.cwd || ""

  for (const project of state?.projects ?? []) {
    const path = project.path || project.name
    const key = path || project.name || "Workspace"
    if (!key || map.has(key)) continue

    map.set(key, {
      name: project.name || basename(path) || "Workspace",
      path,
      sessions: [],
    })
  }

  for (const session of state?.sessions ?? []) {
    const path = session.workspace_path || session.workspace || state?.status.cwd || "Workspace"
    const key = path || session.workspace || "Workspace"
    const current = map.get(key) ?? {
      name: session.workspace || basename(path) || state?.status.workspace || "Workspace",
      path,
      sessions: [],
    }
    map.set(key, { ...current, sessions: [...current.sessions, session] })
  }

  if (currentPath && !map.has(currentPath)) {
    map.set(currentPath, {
      name: state?.status.workspace || basename(currentPath) || "Workspace",
      path: currentPath,
      sessions: [],
    })
  }

  if (map.size === 0 && state?.status.workspace) {
    map.set(state.status.workspace, {
      name: state.status.workspace,
      path: state.status.cwd || state.status.workspace,
      sessions: [],
    })
  }
  return [...map.values()]
}

function PermissionRequestPanel(props: {
  permission: RemotePendingPermission
  busy: boolean
  onAnswer: (response: RemotePermissionResponse) => void
}) {
  const command = () => props.permission.command || props.permission.target || ""
  const queuedText = () => (props.permission.queued_count > 0 ? `+${props.permission.queued_count} queued` : "")

  return (
    <section class="pointer-events-auto mx-auto grid max-h-[min(40vh,18rem)] w-[min(100%,67rem)] overflow-hidden rounded-[16px] border border-[#6f5128] bg-[#211c15]/95 shadow-[0_1rem_3rem_rgba(0,0,0,0.45)] backdrop-blur">
      <div class="grid gap-3 p-4">
        <div class="flex min-w-0 items-start justify-between gap-3">
          <div class="grid min-w-0 gap-1">
            <div class="flex min-w-0 items-center gap-2 text-[#e2b16f]">
              <IconWarningCircle class="h-4 w-4 shrink-0" />
              <h2 class="m-0 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.95rem] font-bold">
                Permission required
              </h2>
              <Show when={queuedText()}>
                {(text) => <span class="rounded-md bg-[#2e261b] px-1.5 py-0.5 text-[0.68rem] font-bold text-[#be9b70]">{text()}</span>}
              </Show>
            </div>
            <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.78rem] text-[var(--muted)]">
              {props.permission.tool_id} / {props.permission.action}
            </div>
          </div>
          <div class="flex shrink-0 gap-2 max-[560px]:hidden">
            <PermissionActionButtons busy={props.busy} onAnswer={props.onAnswer} compact={false} />
          </div>
        </div>

        <div class="grid gap-2 rounded-[10px] border border-[rgba(255,255,255,0.06)] bg-[#171717] p-3">
          <div class="text-[0.82rem] leading-relaxed text-[var(--muted)]">{props.permission.reason}</div>
          <Show when={command()}>
            {(value) => (
              <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.78rem] text-[var(--text)]">
                {value()}
              </div>
            )}
          </Show>
          <Show when={props.permission.workdir}>
            {(workdir) => (
              <div class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.7rem] text-[var(--faint)]">
                {workdir()}
              </div>
            )}
          </Show>
        </div>

        <div class="hidden gap-2 max-[560px]:grid">
          <PermissionActionButtons busy={props.busy} onAnswer={props.onAnswer} compact />
        </div>
      </div>
    </section>
  )
}

function PermissionActionButtons(props: {
  busy: boolean
  compact: boolean
  onAnswer: (response: RemotePermissionResponse) => void
}) {
  return (
    <>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#334d36] bg-[#1d2a1f] px-3 text-[0.82rem] font-bold text-[#a9d6ac] transition hover:bg-[#243326] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("allow_once")}
      >
        <IconCheck class="h-4 w-4" />
        <span>Allow once</span>
      </button>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#2d455f] bg-[#1a2530] px-3 text-[0.82rem] font-bold text-[#9ec8ef] transition hover:bg-[#202d3a] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("allow_always")}
      >
        <IconCheck class="h-4 w-4" />
        <span>Always</span>
      </button>
      <button
        class={cx(
          "inline-flex h-9 items-center justify-center gap-1.5 rounded-lg border border-[#553238] bg-[#2a1c1f] px-3 text-[0.82rem] font-bold text-[#dc9aa2] transition hover:bg-[#332226] disabled:cursor-not-allowed disabled:opacity-55",
          props.compact && "w-full"
        )}
        type="button"
        disabled={props.busy}
        onClick={() => props.onAnswer("deny")}
      >
        <IconX class="h-4 w-4" />
        <span>Reject</span>
      </button>
    </>
  )
}

function QuestionRequestPanel(props: {
  prompt: RemotePendingQuestion
  busy: boolean
  onSubmit: (answers: string[][]) => void
  onCancel: () => void
}) {
  const [selected, setSelected] = createSignal<string[][]>([])
  const [customAnswers, setCustomAnswers] = createSignal<string[]>([])
  let lastPromptKey = ""

  const promptKey = () =>
    props.prompt.questions
      .map((question) =>
        [
          question.header,
          question.question,
          question.multiple ? "multiple" : "single",
          question.custom ? "custom" : "fixed",
          question.options.map((option) => `${option.label}:${option.description}`).join("|"),
        ].join("\u0000")
      )
      .join("\u0001")

  createEffect(() => {
    const key = promptKey()
    if (key === lastPromptKey) return
    lastPromptKey = key
    setSelected(props.prompt.questions.map(() => []))
    setCustomAnswers(props.prompt.questions.map(() => ""))
  })

  const toggleOption = (questionIndex: number, question: RemoteQuestionItem, label: string) => {
    setSelected((current) => {
      const next = current.map((items) => [...items])
      const values = next[questionIndex] ?? []
      if (question.multiple) {
        next[questionIndex] = values.includes(label)
          ? values.filter((item) => item !== label)
          : [...values, label]
      } else {
        next[questionIndex] = values.includes(label) ? [] : [label]
      }
      return next
    })
    if (!question.multiple) {
      setCustomAnswers((current) => current.map((value, index) => (index === questionIndex ? "" : value)))
    }
  }

  const updateCustomAnswer = (questionIndex: number, value: string, question: RemoteQuestionItem) => {
    setCustomAnswers((current) => current.map((item, index) => (index === questionIndex ? value : item)))
    if (!question.multiple && value.trim()) {
      setSelected((current) => current.map((items, index) => (index === questionIndex ? [] : items)))
    }
  }

  const answers = createMemo(() =>
    props.prompt.questions.map((question, index) => {
      const custom = (customAnswers()[index] ?? "").trim()
      if (!question.multiple && custom) return [custom]
      const values = [...(selected()[index] ?? [])]
      if (custom) values.push(custom)
      return values
    })
  )

  const canSubmit = createMemo(() => answers().every((answer) => answer.length > 0))
  const queuedText = () => (props.prompt.queued_count > 0 ? `+${props.prompt.queued_count} queued` : "")

  const submit = (event: SubmitEvent) => {
    event.preventDefault()
    if (!canSubmit() || props.busy) return
    props.onSubmit(answers())
  }

  return (
    <form
      class="pointer-events-auto mx-auto grid max-h-[min(48vh,26rem)] w-[min(100%,67rem)] overflow-hidden rounded-[16px] border border-[#33475f] bg-[#171d24]/95 shadow-[0_1rem_3rem_rgba(0,0,0,0.45)] backdrop-blur"
      onSubmit={submit}
    >
      <div class="flex min-w-0 items-center justify-between gap-3 border-b border-[rgba(255,255,255,0.07)] px-4 py-3">
        <div class="flex min-w-0 items-center gap-2 text-[#9ec8ef]">
          <IconBrainGlyph class="h-4 w-4 shrink-0" />
          <h2 class="m-0 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.95rem] font-bold">
            Agent needs input
          </h2>
          <Show when={queuedText()}>
            {(text) => <span class="rounded-md bg-[#202a34] px-1.5 py-0.5 text-[0.68rem] font-bold text-[#94b8d8]">{text()}</span>}
          </Show>
        </div>
        <button
          class="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg border border-[#553238] bg-[#261c1f] px-2.5 text-[0.78rem] font-bold text-[#dc9aa2] transition hover:bg-[#302125] disabled:cursor-not-allowed disabled:opacity-55"
          type="button"
          disabled={props.busy}
          onClick={props.onCancel}
        >
          <IconX class="h-3.5 w-3.5" />
          <span class="max-[560px]:hidden">Cancel run</span>
        </button>
      </div>

      <div class="min-h-0 overflow-y-auto px-4 py-3">
        <div class="grid gap-4">
          <For each={props.prompt.questions}>
            {(question, questionIndex) => (
              <fieldset class="grid min-w-0 gap-2 rounded-[10px] border border-[rgba(255,255,255,0.06)] bg-[#151515] p-3">
                <legend class="px-1 text-[0.72rem] font-bold uppercase tracking-[0.07em] text-[var(--faint)]">
                  {question.header || `Question ${questionIndex() + 1}`}
                </legend>
                <div class="text-[0.92rem] leading-relaxed text-[var(--text)]">{question.question}</div>
                <Show when={question.multiple}>
                  <div class="text-[0.74rem] text-[var(--faint)]">Choose one or more.</div>
                </Show>

                <Show when={question.options.length > 0}>
                  <div class="grid gap-1.5">
                    <For each={question.options}>
                      {(option) => {
                        const checked = createMemo(() => (selected()[questionIndex()] ?? []).includes(option.label))
                        return (
                          <label class="grid min-w-0 cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-start gap-2 rounded-[8px] px-2 py-1.5 text-[var(--muted)] hover:bg-white/[0.045]">
                            <input
                              class="mt-1 accent-[#9ec8ef]"
                              type={question.multiple ? "checkbox" : "radio"}
                              name={`remote-question-${questionIndex()}`}
                              checked={checked()}
                              onChange={() => toggleOption(questionIndex(), question, option.label)}
                            />
                            <span class="grid min-w-0 gap-0.5">
                              <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold text-[var(--text)]">
                                {option.label}
                              </span>
                              <Show when={option.description}>
                                <span class="text-[0.76rem] leading-snug text-[var(--faint)]">{option.description}</span>
                              </Show>
                            </span>
                          </label>
                        )
                      }}
                    </For>
                  </div>
                </Show>

                <Show when={question.custom}>
                  <input
                    class={cx(INPUT_BASE, "h-10 text-[0.86rem]")}
                    value={customAnswers()[questionIndex()] ?? ""}
                    onInput={(event) => updateCustomAnswer(questionIndex(), event.currentTarget.value, question)}
                    placeholder={question.options.length > 0 ? "Or type your own answer" : "Type your answer"}
                  />
                </Show>
              </fieldset>
            )}
          </For>
        </div>
      </div>

      <div class="flex items-center justify-end gap-2 border-t border-[rgba(255,255,255,0.07)] px-4 py-3">
        <button
          class="h-9 rounded-lg bg-[#e5e2dc] px-4 text-[0.84rem] font-bold text-[#171717] transition hover:bg-[#f0ede7] disabled:cursor-not-allowed disabled:opacity-45"
          type="submit"
          disabled={!canSubmit() || props.busy}
        >
          Submit answer
        </button>
      </div>
    </form>
  )
}

function ThreadItemView(props: {
  item: Accessor<ThreadItem>
  status: Accessor<RemoteStatus | null>
  token: Accessor<string>
  onPreviewImage: (attachment: AttachmentData) => void
}) {
  const message = createMemo(() => {
    const item = props.item()
    return item.type === "message" ? item.message : null
  })
  const messageActivityTools = createMemo(() => {
    const item = props.item()
    return item.type === "message" ? item.activityTools : []
  })
  const activity = createMemo(() => {
    const item = props.item()
    return item.type === "activity" ? item.tools : null
  })
  const action = createMemo(() => {
    const item = props.item()
    return item.type === "action" ? item.tool : null
  })

  return (
    <>
      <Show when={message()}>
        {(current) => (
          <MessageView
            message={current}
            activityTools={messageActivityTools}
            status={props.status}
            token={props.token}
            onPreviewImage={props.onPreviewImage}
          />
        )}
      </Show>
      <Show when={activity()}>{(tools) => <ToolActivityGroup tools={tools} />}</Show>
      <Show when={action()}>{(tool) => <ToolActionMessage tool={tool} />}</Show>
    </>
  )
}

function ToolActivityGroup(props: { tools: Accessor<ToolMessage[]> }) {
  const steps = createMemo(() => buildActivitySteps(props.tools()))
  const state = createMemo<ToolVisualState>(() => {
    if (steps().some((step) => step.state === "error")) return "error"
    if (steps().some((step) => step.state === "active")) return "active"
    return "complete"
  })

  return (
    <article class="grid grid-cols-[2rem_minmax(0,1fr)] gap-3 py-1">
      <div class="w-7" />
      <div class="w-[min(100%,44rem)] min-w-0">
        <ToolActivityTimeline steps={steps} state={state} />
      </div>
    </article>
  )
}

function ToolActivityTimeline(props: {
  steps: Accessor<ToolActivityStep[]>
  state: Accessor<ToolVisualState>
}) {
  return (
    <section class="flex w-[min(100%,30rem)] flex-col text-[#d8d6d1]" aria-label="Tool activity">
      <For each={props.steps()}>{(step) => <ToolTimelineStep step={step} />}</For>
      <Show when={props.state() === "complete"}>
        <ToolTimelineStep
          step={{
            key: "done",
            label: "Done",
            icon: "check",
            state: "complete",
            details: [],
          }}
        />
      </Show>
    </section>
  )
}

function ToolTimelineStep(props: { step: ToolActivityStep }) {
  const [open, setOpen] = createSignal(props.step.defaultOpen ?? false)
  const hasDetails = () => props.step.details.length > 0 || Boolean(props.step.preview)

  return (
    <div class="relative grid min-w-0 grid-cols-[1.7rem_minmax(0,1fr)] gap-3 py-1 before:absolute before:top-[1.35rem] before:-bottom-1 before:left-[0.85rem] before:w-px before:-translate-x-1/2 before:rounded-full before:bg-[var(--line-strong)] before:content-[''] last:before:hidden">
      <div class="relative grid h-[1.7rem] w-[1.7rem] place-items-center text-[var(--muted)]">
        <ToolIcon
          kind={props.step.icon}
          class={cx("relative z-[1] h-[1.08rem] w-[1.08rem]", toolStateClass(props.step.state))}
        />
      </div>
      <div class="min-w-0 pb-1">
        <button
          type="button"
          class="inline-flex min-w-0 max-w-full items-center gap-1.5 py-0.5 text-left text-[14px] font-medium leading-snug text-[#dedbd4] disabled:cursor-default [&[aria-expanded=true]_.tool-chevron]:rotate-180"
          disabled={!hasDetails()}
          aria-expanded={open()}
          onClick={() => hasDetails() && setOpen((value) => !value)}
        >
          <span class="min-w-0 [overflow-wrap:anywhere]">{props.step.label}</span>
          <Show when={hasDetails()}>
            <IconCaretDown class="tool-chevron h-3 w-3 shrink-0 text-[var(--faint)] transition-transform duration-150" />
          </Show>
        </button>
        <Show when={hasDetails()}>
          <CollapsiblePanel open={open()} class="w-full">
            <ToolDetails details={props.step.details} preview={props.step.preview} compact />
          </CollapsiblePanel>
        </Show>
      </div>
    </div>
  )
}

function ToolActionMessage(props: { tool: Accessor<ToolMessage> }) {
  const descriptor = createMemo(() => actionDescriptor(props.tool()))
  const [open, setOpen] = createSignal(descriptor().state !== "complete")

  createEffect(() => {
    if (descriptor().state === "active" || descriptor().state === "error") setOpen(true)
  })

  return (
    <article class="py-1">
      <div class="w-[min(100%,44rem)] min-w-0">
        <section
          class={cx(
            "w-[min(100%,44rem)] overflow-hidden rounded-lg border border-[var(--line)] bg-white/[0.028]",
            descriptor().state === "active" && "border-[rgba(108,142,216,0.28)] bg-[rgba(108,142,216,0.055)]",
            descriptor().state === "error" && "border-[rgba(200,108,116,0.3)] bg-[rgba(200,108,116,0.07)]"
          )}
        >
          <button
            type="button"
            class="grid w-full min-w-0 grid-cols-[1.85rem_minmax(0,1fr)_auto_auto] items-center gap-3 px-3 py-3 text-left hover:bg-white/[0.035] [&[aria-expanded=true]_.tool-chevron]:rotate-180"
            aria-expanded={open()}
            onClick={() => setOpen((value) => !value)}
          >
            <span class="grid h-[1.85rem] w-[1.85rem] place-items-center rounded-md bg-white/[0.04] text-[var(--muted)]">
              <ToolIcon
                kind={descriptor().icon}
                class={cx("h-[1.05rem] w-[1.05rem]", toolStateClass(descriptor().state))}
              />
            </span>
            <span class="flex min-w-0 flex-col gap-0.5">
              <strong class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[14px] font-semibold leading-tight text-[var(--text)]">
                {descriptor().label}
              </strong>
              <small class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.72rem] leading-snug text-[var(--muted)]">
                {descriptor().description}
              </small>
            </span>
            <Show when={descriptor().stats}>
              {(stats) => (
                <span class="inline-flex items-center gap-1.5 whitespace-nowrap font-mono text-[0.73rem] font-semibold text-[var(--muted)]" aria-label="Diff summary">
                  <span class="text-[#7fc99e]">+{stats().added}</span>
                  <span class="text-[#da8b92]">-{stats().removed}</span>
                </span>
              )}
            </Show>
            <IconCaretDown class="tool-chevron h-3.5 w-3.5 text-[var(--faint)] transition-transform duration-150" />
          </button>
          <CollapsiblePanel open={open()} class="border-t border-[var(--line)]">
            <ToolDetails
              details={descriptor().details}
              preview={descriptor().preview}
              diffLines={descriptor().diffLines}
            />
          </CollapsiblePanel>
        </section>
      </div>
    </article>
  )
}

function ToolDetails(props: {
  details: ToolStepDetail[]
  preview?: string
  diffLines?: DiffLine[]
  compact?: boolean
}) {
  return (
    <div class={cx("flex min-w-0 flex-col gap-3 px-3 py-3", props.compact && "gap-2 px-0 pt-2 pb-1")}>
      <Show when={props.details.length > 0}>
        <div class="flex min-w-0 flex-col gap-2">
          <For each={props.details}>
            {(detail) => (
              <div class="grid min-w-0 grid-cols-[0.8rem_minmax(0,1fr)] items-start gap-2">
                <span
                  class={cx(
                    "mt-1.5 h-[0.42rem] w-[0.42rem] rounded-full border border-[var(--line-strong)] bg-white/10",
                    detail.status === "active" && "border-[rgba(108,142,216,0.65)] bg-[var(--brand-primary)] animate-toolPulse",
                    detail.status === "error" && "border-[rgba(200,108,116,0.65)] bg-[var(--red)]",
                    (detail.status === "complete" || !detail.status) && "border-[rgba(92,168,134,0.5)] bg-[var(--green)]"
                  )}
                  aria-hidden="true"
                />
                <span class="flex min-w-0 flex-col gap-0.5">
                  <strong class="min-w-0 text-[0.79rem] font-medium leading-snug text-[#d7d5d0] [overflow-wrap:anywhere]">
                    {detail.label}
                  </strong>
                  <Show when={detail.detail}>
                    {(detailText) => (
                      <small class="min-w-0 text-[0.73rem] leading-snug text-[var(--faint)] [overflow-wrap:anywhere]">
                        {detailText()}
                      </small>
                    )}
                  </Show>
                </span>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={props.diffLines && props.diffLines.length > 0}>
        <div class="grid max-w-full gap-0.5 overflow-x-auto rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]" aria-label="Diff preview">
          <For each={props.diffLines}>
            {(line) => (
              <div
                class={cx(
                  "grid min-w-max grid-cols-[1rem_minmax(0,1fr)] gap-2",
                  line.kind === "add" && "text-[#8fd8aa]",
                  line.kind === "remove" && "text-[#e09299]"
                )}
              >
                <span class="select-none text-[var(--faint)]">{line.kind === "add" ? "+" : line.kind === "remove" ? "-" : " "}</span>
                <code class="[font:inherit] text-inherit">{line.text}</code>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={props.preview}>
        {(preview) => (
          <pre class="m-0 max-w-full overflow-x-auto whitespace-pre rounded-[7px] border border-[var(--line)] bg-black/20 p-3 font-mono text-[0.73rem] leading-normal text-[#bebbb4]">
            {trimPreview(preview())}
          </pre>
        )}
      </Show>
    </div>
  )
}

function ToolIcon(props: { kind: ToolIconKind; class?: string }) {
  if (props.kind === "check") return <IconCheck class={props.class} />
  if (props.kind === "file") return <IconFileText class={props.class} />
  if (props.kind === "globe") return <IconGlobe class={props.class} />
  if (props.kind === "pencil") return <IconPencilSimple class={props.class} />
  if (props.kind === "search") return <IconSearch class={props.class} />
  if (props.kind === "terminal") return <IconTerminal class={props.class} />
  if (props.kind === "warning") return <IconWarningCircle class={props.class} />
  return <IconBrainGlyph class={props.class} />
}

function toolStateClass(state: ToolVisualState) {
  if (state === "active") return "text-[var(--brand-primary)] animate-toolPulse"
  if (state === "error") return "text-[var(--red)]"
  return "text-[#bcb9b1]"
}

function MessageView(props: {
  message: Accessor<RemoteMessage>
  activityTools: Accessor<ToolMessage[]>
  status: Accessor<RemoteStatus | null>
  token: Accessor<string>
  onPreviewImage: (attachment: AttachmentData) => void
}) {
  const isUser = () => props.message().role === "user"
  const userAttachments = createMemo(() => messageImageAttachmentData(props.message(), props.token()))
  const hasThoughtProcess = () =>
    Boolean(props.message().reasoning?.trim()) || props.activityTools().length > 0
  const showAssistantBubble = () =>
    props.message().content.trim().length > 0 || (!props.message().is_complete && props.activityTools().length === 0)
  const showStreamingPlaceholder = () =>
    !isUser() && !props.message().is_complete && !props.message().content.trim()
  const copyContent = () => props.message().content || ""
  return (
    <Message from={props.message().role} class={cx(!isUser() && "w-full items-stretch")}>
      <MessageContent class={cx("w-full", isUser() && "flex flex-col items-end")}>
        <Show when={hasThoughtProcess()}>
          <ThinkingAccordion
            text={props.message().reasoning || ""}
            activityTools={props.activityTools}
            streaming={!props.message().is_complete}
          />
        </Show>
        <Show
          when={isUser()}
          fallback={
            <>
              <Show when={showAssistantBubble()}>
                <div class="mt-1 w-full whitespace-normal break-words pl-2 text-[0.95rem] leading-relaxed text-[#d7d5d0]">
                  <Show
                    when={showStreamingPlaceholder()}
                    fallback={<MessageResponse content={props.message().content} />}
                  >
                    <Shimmer class="text-[0.95rem] leading-relaxed" duration={1.6}>
                      Working...
                    </Shimmer>
                  </Show>
                </div>
              </Show>
              <MessageToolbar class="mt-2 w-full justify-start pl-2">
                <AssistantMetadata message={props.message} status={props.status} />
              </MessageToolbar>
              <MessageActions class="mt-1">
                <CopyMessageAction content={copyContent} />
              </MessageActions>
            </>
          }
        >
          <Show when={userAttachments().length > 0}>
            <Attachments
              variant="grid"
              class="ml-auto mt-1 !flex max-w-[min(100%,42rem)] flex-wrap justify-end gap-2"
            >
              <For each={userAttachments()}>
                {(attachment) => (
                  <Attachment
                    data={attachment}
                    class="w-[min(14rem,calc(100vw-2rem))] cursor-zoom-in transition hover:border-[rgba(255,255,255,0.16)] hover:bg-[#242424] focus-visible:border-[rgba(157,177,239,0.55)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[rgba(157,177,239,0.18)]"
                    role="button"
                    tabIndex={0}
                    onClick={() => props.onPreviewImage(attachment)}
                    onKeyDown={(event) => handleImagePreviewKeyDown(event, () => props.onPreviewImage(attachment))}
                  >
                    <AttachmentPreview />
                    <div class="px-2 py-1.5">
                      <AttachmentInfo />
                    </div>
                  </Attachment>
                )}
              </For>
            </Attachments>
          </Show>
          <div class="ml-auto mt-1 w-fit max-w-[min(100%,42rem)] whitespace-pre-wrap break-words rounded-[12px_12px_4px_12px] border border-[var(--line)] bg-[#232323] px-3 py-2 text-[0.95rem] leading-relaxed text-[var(--text)]">
            <For each={promptTextParts(props.message().content || "Working...", userAttachments().length)}>
              {(part) => (
                <span
                  class={promptTextPartClass(part)}
                  style={promptTextPartStyle(part)}
                >
                  {part.text}
                </span>
              )}
            </For>
          </div>
          <MessageActions class="mt-1 justify-end">
            <CopyMessageAction content={copyContent} />
          </MessageActions>
        </Show>
      </MessageContent>
    </Message>
  )
}

function ImagePreviewDialog(props: {
  image: Accessor<ImagePreviewTarget | null>
  onClose: () => void
}) {
  createEffect(() => {
    if (!props.image()) return

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      props.onClose()
    }

    window.addEventListener("keydown", onKeyDown)
    onCleanup(() => window.removeEventListener("keydown", onKeyDown))
  })

  return (
    <Show when={props.image()}>
      {(image) => (
        <div
          class="fixed inset-0 z-[140] grid place-items-center bg-black/80 p-2 animate-fadeIn"
          role="dialog"
          aria-modal="true"
          aria-label={image().label}
          onMouseDown={(event) => event.currentTarget === event.target && props.onClose()}
        >
          <button
            class="absolute top-3 right-3 z-[1] grid h-8 w-8 place-items-center rounded-full bg-black/40 text-[#d9d7d0] backdrop-blur transition hover:bg-black/60 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/30"
            type="button"
            aria-label="Close image preview"
            onClick={props.onClose}
          >
            <IconX class="h-4 w-4" />
          </button>
          <img
            src={image().url}
            alt={image().label}
            class="block max-h-[calc(100dvh-1rem)] max-w-[calc(100vw-1rem)] rounded-[6px] bg-[#101010] object-contain shadow-[0_1rem_4rem_rgba(0,0,0,0.55)]"
          />
        </div>
      )}
    </Show>
  )
}

function AssistantMetadata(props: {
  message: Accessor<RemoteMessage>
  status: Accessor<RemoteStatus | null>
}) {
  const agent = () => displayAgentMode(props.message().agent_mode || props.status()?.agent || "Build")
  const model = () => messageModelLabel(props.message(), props.status())
  const metrics = () => assistantMetrics(props.message())
  const accent = () => agentAccentClass(agent())

  return (
    <div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[0.76rem] leading-snug text-[#8d93bd]">
      <span class={cx("h-2.5 w-2.5 shrink-0 border", accent())} aria-hidden="true" />
      <span class={cx("font-bold", accent())}>{agent()}</span>
      <span class="text-[#686b86]">•</span>
      <span class="min-w-0 max-w-full overflow-hidden text-ellipsis whitespace-nowrap text-[#a4a8cc]">
        {model()}
      </span>
      <For each={metrics()}>
        {(metric) => (
          <>
            <span class="text-[#686b86]">•</span>
            <span class="text-[#8d93bd]">{metric}</span>
          </>
        )}
      </For>
    </div>
  )
}

function CopyMessageAction(props: { content: Accessor<string> }) {
  const [copied, setCopied] = createSignal(false)
  let timer: number | undefined

  const copy = async () => {
    const text = props.content()
    if (!text.trim()) return
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      try {
        fallbackCopyText(text)
      } catch {
        // The visual acknowledgement still matters in restricted browser contexts.
      }
    }
    setCopied(true)
    if (timer) window.clearTimeout(timer)
    timer = window.setTimeout(() => setCopied(false), 1800)
  }

  onCleanup(() => {
    if (timer) window.clearTimeout(timer)
  })

  return (
    <MessageAction
      label={copied() ? "Copied" : "Copy"}
      disabled={!props.content().trim()}
      onClick={copy}
    >
      <Show
        when={copied()}
        fallback={<IconCopy class="h-3.5 w-3.5 animate-scaleIn" />}
      >
        <IconCheck class="h-3.5 w-3.5 animate-scaleIn text-[var(--green)]" />
      </Show>
    </MessageAction>
  )
}

function ThinkingAccordion(props: {
  text: string
  activityTools: Accessor<ToolMessage[]>
  streaming: boolean
}) {
  const steps = createMemo(() => buildActivitySteps(props.activityTools()))
  const state = createMemo<ToolVisualState>(() => {
    if (steps().some((step) => step.state === "error")) return "error"
    if (steps().some((step) => step.state === "active")) return "active"
    return "complete"
  })
  const hasActivity = () => props.activityTools().length > 0
  const [open, setOpen] = createSignal(props.streaming || hasActivity())
  let hasAutoClosed = false

  createEffect(() => {
    if (props.streaming || state() === "active" || state() === "error") {
      hasAutoClosed = false
      setOpen(true)
      return
    }

    if (open() && !hasAutoClosed && !hasActivity()) {
      const timer = window.setTimeout(() => {
        hasAutoClosed = true
        setOpen(false)
      }, 1000)
      onCleanup(() => window.clearTimeout(timer))
    }
  })

  return (
    <div class="mt-2 w-[min(100%,42rem)] text-[#c9c6bf]">
      <button
        class="inline-flex min-h-[1.9rem] items-center gap-2 rounded-full px-2 py-1 text-[14px] font-medium text-[var(--muted)] transition hover:bg-white/[0.045] hover:text-[var(--text)] [&[aria-expanded=true]_.thinking-chevron]:rotate-180"
        type="button"
        aria-expanded={open()}
        onClick={() => setOpen((value) => !value)}
      >
        <IconBrainGlyph class="h-4 w-4 text-[var(--faint)]" />
        <span>{props.streaming ? "Thinking" : "Thought process"}</span>
        <IconCaretDown class="thinking-chevron h-3 w-3 text-[var(--faint)] transition-transform duration-150" />
      </button>
      <CollapsiblePanel open={open()} class="w-full">
        <div class="w-full overflow-x-auto pl-4 pt-1 text-[14px] leading-relaxed text-[var(--muted)]">
          <Show when={props.text.trim()}>
            <StreamMarkdown content={props.text} class="streamdown remote-markdown text-[var(--muted)]" />
          </Show>
          <Show when={hasActivity()}>
            <div class="mt-1 [&_.tool-activity]:w-[min(100%,34rem)]">
              <ToolActivityTimeline steps={steps} state={state} />
            </div>
          </Show>
        </div>
      </CollapsiblePanel>
    </div>
  )
}

function buildThreadItems(messages: RemoteMessage[], cwd: string): ThreadItem[] {
  const items: ThreadItem[] = []
  let activeAssistantItem: Extract<ThreadItem, { type: "message" }> | null = null
  let orphanActivityTools: ToolMessage[] = []

  const flushOrphanActivity = () => {
    if (orphanActivityTools.length === 0) return
    items.push({ type: "activity", tools: orphanActivityTools })
    orphanActivityTools = []
  }

  for (const message of messages) {
    if (message.role !== "tool") {
      if (message.role === "assistant" && activeAssistantItem) {
        activeAssistantItem.message = mergeAssistantTurnMessages(
          activeAssistantItem.message,
          message
        )
        continue
      }

      flushOrphanActivity()
      const item: ThreadItem = { type: "message", message, activityTools: [] }
      items.push(item)
      activeAssistantItem = message.role === "assistant" ? item : null
      if (activeAssistantItem) {
        for (const tool of assistantPartToolMessages(message, cwd)) {
          if (isActivityTool(tool)) {
            activeAssistantItem.activityTools.push(tool)
          } else {
            flushOrphanActivity()
            items.push({ type: "action", tool })
          }
        }
      }
      continue
    }

    const tool = parseToolMessage(message, cwd)
    if (isActivityTool(tool)) {
      if (activeAssistantItem) {
        activeAssistantItem.activityTools.push(tool)
      } else {
        orphanActivityTools.push(tool)
      }
    } else {
      flushOrphanActivity()
      items.push({ type: "action", tool })
    }
  }

  flushOrphanActivity()
  return items
}

function assistantPartToolMessages(message: RemoteMessage, cwd: string): ToolMessage[] {
  const parts = Array.isArray(message.parts) ? message.parts : []
  if (parts.length === 0) return []

  const resultIds = new Set(
    parts
      .filter((part) => part.type === "tool_result")
      .map(toolPartId)
      .filter((id): id is string => Boolean(id))
  )
  const callsById = new Map<string, RemoteMessagePart>()
  const tools: ToolMessage[] = []

  for (const part of parts) {
    if (part.type === "tool_call") {
      const id = toolPartId(part)
      if (id) callsById.set(id, part)
      if (!id || resultIds.has(id)) continue
      const payload = { ...toolPartPayload(part), status: stringValue(part.status) || "running" }
      tools.push(toolMessageFromPayload(message, payload, cwd))
      continue
    }

    if (part.type === "tool_result") {
      const id = toolPartId(part)
      const call = id ? callsById.get(id) : undefined
      const payload = toolPartPayload(part)
      if (payload.args === undefined && call?.args !== undefined) payload.args = call.args
      tools.push(toolMessageFromPayload(message, payload, cwd))
    }
  }

  return tools
}

function toolPartId(part: RemoteMessagePart): string | null {
  return stringValue(part.id) || stringValue(part.call_id)
}

function toolPartPayload(part: RemoteMessagePart): JsonObject {
  const payload: JsonObject = {}
  for (const [key, value] of Object.entries(part)) {
    if (key === "type") continue
    payload[key] = value
  }
  return payload
}

function toolMessageFromPayload(
  baseMessage: RemoteMessage,
  payload: JsonObject,
  cwd: string
): ToolMessage {
  const toolMessage: RemoteMessage = {
    ...baseMessage,
    role: "tool",
    content: JSON.stringify(payload),
    reasoning: null,
    parts: [],
  }
  return parseToolMessage(toolMessage, cwd)
}

function mergeAssistantTurnMessages(base: RemoteMessage, next: RemoteMessage): RemoteMessage {
  return {
    ...base,
    content: joinMessageParts(base.content, next.content),
    reasoning: joinOptionalMessageParts(base.reasoning, next.reasoning),
    parts: [...(base.parts || []), ...(next.parts || [])],
    is_complete: next.is_complete,
    agent_mode: next.agent_mode ?? base.agent_mode,
    token_count: next.token_count ?? base.token_count,
    duration_ms: next.duration_ms ?? base.duration_ms,
    t0_ms: next.t0_ms ?? base.t0_ms,
    t1_ms: next.t1_ms ?? base.t1_ms,
    tn_ms: next.tn_ms ?? base.tn_ms,
    output_tokens: next.output_tokens ?? base.output_tokens,
    model: next.model ?? base.model,
    provider: next.provider ?? base.provider,
    local_image_paths: [...base.local_image_paths, ...next.local_image_paths],
    was_interrupted: base.was_interrupted || next.was_interrupted,
  }
}

function joinOptionalMessageParts(
  first: string | null,
  second: string | null
): string | null {
  const joined = joinMessageParts(first || "", second || "")
  return joined.length > 0 ? joined : null
}

function joinMessageParts(first: string, second: string): string {
  const left = first.trimEnd()
  const right = second.trimStart()
  if (!left) return right
  if (!right) return left
  return `${left}\n\n${right}`
}

function parseToolMessage(message: RemoteMessage, cwd: string): ToolMessage {
  const obj = parseJsonObject(message.content)
  const parsed: ParsedToolMessage = {
    id: stringValue(obj?.id) || stringValue(obj?.call_id) || cuid(),
    name: stringValue(obj?.name) || "tool",
    status: stringValue(obj?.status) || "ok",
    args: obj?.args,
    metadata: obj?.metadata,
    outputPreview: stringValue(obj?.output_preview),
    title: stringValue(obj?.title),
    lineCount: numberValue(obj?.line_count),
  }

  if (!obj && message.content.trim()) {
    parsed.outputPreview = message.content
  }

  return { message, parsed, cwd }
}

function parseJsonObject(content: string): JsonObject | null {
  try {
    const value = JSON.parse(content) as JsonValue
    return asObject(value) ?? null
  } catch {
    return null
  }
}

function isActivityTool(tool: ToolMessage) {
  return THINKING_TOOL_NAMES.has(tool.parsed.name) && !ACTION_TOOL_NAMES.has(tool.parsed.name)
}

function buildActivitySteps(tools: ToolMessage[]): ToolActivityStep[] {
  const steps: ToolActivityStep[] = []
  let exploration: ToolMessage[] = []

  const flushExploration = () => {
    if (exploration.length === 0) return
    steps.push(explorationActivityStep(exploration, steps.length))
    exploration = []
  }

  for (const tool of tools) {
    if (EXPLORATION_TOOL_NAMES.has(tool.parsed.name)) {
      exploration.push(tool)
    } else {
      flushExploration()
      steps.push(activityStepFromTool(tool, steps.length))
    }
  }

  flushExploration()
  return steps
}

function explorationActivityStep(tools: ToolMessage[], index: number): ToolActivityStep {
  const details = tools.map(explorationDetail)
  const state = combinedToolState(tools)
  const count = Math.max(1, details.length)
  const first = details[0]?.label ?? "Explored files"
  const label =
    state === "active"
      ? count === 1
        ? first.replace(/^Read /, "Reading ").replace(/^Listed /, "Listing ").replace(/^Searched /, "Searching ")
        : `Exploring ${formatCount(count, "file")}`
      : state === "error"
        ? count === 1
          ? `${first} failed`
          : "File exploration failed"
        : count === 1
          ? first
          : `Explored ${formatCount(count, "file")}`

  return {
    key: `exploration-${index}-${tools.map((tool) => tool.parsed.id).join("-")}`,
    label,
    icon: "search",
    state,
    details,
    defaultOpen: state !== "complete" || details.length > 1,
  }
}

function explorationDetail(tool: ToolMessage): ToolStepDetail {
  const args = asObject(tool.parsed.args)
  const title = tool.parsed.title
  const status = toolState(tool)

  if (tool.parsed.name === "read") {
    const path = argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(title, "Read")
    return {
      label: `Read ${displayPath(path || "file", tool.cwd, true)}`,
      detail: firstPreviewLine(tool.parsed.outputPreview),
      status,
    }
  }

  if (tool.parsed.name === "list") {
    const path = argString(args, ["path"]) || stripToolTitle(title, "List") || "."
    return {
      label: `Listed ${displayPath(path, tool.cwd, false)}`,
      detail: firstPreviewLine(tool.parsed.outputPreview),
      status,
    }
  }

  const query =
    argString(args, ["pattern", "query"]) ||
    stripToolTitle(title, tool.parsed.name === "glob" ? "Glob" : "Grep") ||
    "workspace"
  const path = argString(args, ["path"])
  const include = argString(args, ["include"])
  return {
    label: `Searched ${query}`,
    detail: [path ? displayPath(path, tool.cwd, false) : "", include ? `include=${include}` : ""]
      .filter(Boolean)
      .join(" "),
    status,
  }
}

function activityStepFromTool(tool: ToolMessage, index: number): ToolActivityStep {
  const args = asObject(tool.parsed.args)
  const metadata = asObject(tool.parsed.metadata)
  const state = toolState(tool)
  const key = `${tool.parsed.name}-${tool.parsed.id}-${index}`

  if (tool.parsed.name === "webfetch") {
    const url =
      argString(metadata, ["url"]) ||
      argString(args, ["url"]) ||
      stripToolTitle(tool.parsed.title, "Fetched") ||
      "source"
    return {
      key,
      label: state === "active" ? "Searching web" : state === "error" ? "Web search failed" : "Searched web",
      icon: "globe",
      state,
      details: [{ label: readableUrl(url), detail: firstPreviewLine(tool.parsed.outputPreview), status: state }],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "view_image") {
    const path = argString(metadata, ["path"]) || argString(args, ["path"]) || "image"
    const width = numberValue(metadata?.width)
    const height = numberValue(metadata?.height)
    return {
      key,
      label: state === "active" ? "Viewing image" : state === "error" ? "Image view failed" : "Viewed image",
      icon: "file",
      state,
      details: [
        {
          label: displayPath(path, tool.cwd, true),
          detail: width && height ? `${width} x ${height}` : undefined,
          status: state,
        },
      ],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "skill") {
    const name = argString(metadata, ["name"]) || argString(args, ["name"]) || stripToolTitle(tool.parsed.title, "Loaded skill")
    const resources = arrayValue(metadata?.resources)
    return {
      key,
      label:
        state === "active"
          ? `Loading skill${name ? ` ${name}` : ""}`
          : state === "error"
            ? "Skill load failed"
            : `Loaded skill${name ? ` ${name}` : ""}`,
      icon: "brain",
      state,
      details: resources.length > 0 ? [{ label: formatCount(resources.length, "resource"), status: state }] : [],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "task") {
    const subagent = argString(metadata, ["subagent_type"]) || argString(args, ["subagent_type"]) || "agent"
    const description =
      argString(metadata, ["child_session_title"]) || argString(args, ["description"]) || firstPreviewLine(tool.parsed.outputPreview)
    return {
      key,
      label:
        state === "active"
          ? `Running ${formatToolName(subagent)} agent`
          : state === "error"
            ? `${formatToolName(subagent)} agent failed`
            : `Ran ${formatToolName(subagent)} agent`,
      icon: "brain",
      state,
      details: description ? [{ label: description, status: state }] : [],
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "update_plan" || tool.parsed.name === "todowrite") {
    const planDetails = planStepDetails(tool)
    return {
      key,
      label: state === "active" ? "Updating plan" : state === "error" ? "Plan update failed" : "Updated plan",
      icon: "check",
      state,
      details: planDetails,
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  if (tool.parsed.name === "question") {
    const questions = questionDetails(tool)
    return {
      key,
      label:
        state === "active"
          ? formatCount(Math.max(questions.length, 1), "question", "Asking")
          : state === "error"
            ? "Question failed"
            : formatCount(Math.max(questions.length, 1), "question", "Answered"),
      icon: "brain",
      state,
      details: questions,
      preview: state === "error" ? tool.parsed.outputPreview : undefined,
      defaultOpen: state !== "complete",
    }
  }

  return {
    key,
    label:
      state === "active"
        ? `Running ${formatToolName(tool.parsed.name)}`
        : state === "error"
          ? `${formatToolName(tool.parsed.name)} failed`
          : formatToolName(tool.parsed.title || tool.parsed.name),
    icon: state === "error" ? "warning" : "brain",
    state,
    details: genericToolDetails(tool),
    preview: tool.parsed.outputPreview,
    defaultOpen: state !== "complete",
  }
}

function actionDescriptor(tool: ToolMessage): ActionDescriptor {
  const state = toolState(tool)
  const args = asObject(tool.parsed.args)
  const metadata = asObject(tool.parsed.metadata)
  const errorPreview = state === "error" ? tool.parsed.outputPreview : undefined

  if (tool.parsed.name === "edit") {
    const filePath =
      argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(tool.parsed.title, "Edit") || "file"
    const oldText = argString(args, ["old_string", "oldString"]) || ""
    const newText = argString(args, ["new_string", "newString"]) || ""
    return {
      label: state === "active" ? "Editing" : state === "error" ? "Edit failed" : "Edited",
      description: displayPath(filePath, tool.cwd, false),
      state,
      icon: state === "error" ? "warning" : "pencil",
      stats: diffStats(oldText, newText),
      details: [
        {
          label: displayPath(filePath, tool.cwd, false),
          detail: lineNumberDetail(metadata, tool.parsed.outputPreview),
          status: state,
        },
      ],
      diffLines: compactDiffLines(diffLineOps(oldText, newText)),
      preview: errorPreview,
    }
  }

  if (tool.parsed.name === "write") {
    const filePath =
      argString(args, ["file_path", "filePath", "path"]) || stripToolTitle(tool.parsed.title, "Write") || "file"
    const newText = argString(args, ["content"]) || ""
    const created = tool.parsed.outputPreview?.startsWith("Created file")
    return {
      label: state === "active" ? "Writing" : state === "error" ? "Write failed" : created ? "Added" : "Edited",
      description: displayPath(filePath, tool.cwd, false),
      state,
      icon: state === "error" ? "warning" : "pencil",
      stats: diffStats("", newText),
      details: [
        {
          label: displayPath(filePath, tool.cwd, false),
          detail: firstPreviewLine(tool.parsed.outputPreview),
          status: state,
        },
      ],
      diffLines: compactDiffLines(diffLineOps("", newText)),
      preview: errorPreview,
    }
  }

  if (tool.parsed.name === "apply_patch") {
    const patch = argString(args, ["patch"]) || ""
    const paths = patchPaths(patch, tool.cwd)
    const fileCount = numberValue(metadata?.file_count) ?? paths.length
    return {
      label: state === "active" ? "Applying patch" : state === "error" ? "Patch failed" : "Applied patch",
      description: fileCount > 0 ? formatCount(fileCount, "file") : tool.parsed.title || "Workspace patch",
      state,
      icon: state === "error" ? "warning" : "pencil",
      details:
        paths.length > 0
          ? paths.slice(0, 8).map((path) => ({ label: path, status: state }))
          : [{ label: tool.parsed.title || "Patch", detail: firstPreviewLine(tool.parsed.outputPreview), status: state }],
      diffLines: [],
      preview: state === "error" ? tool.parsed.outputPreview : firstPreviewLine(tool.parsed.outputPreview),
    }
  }

  if (tool.parsed.name === "bash") {
    const command = argString(metadata, ["command"]) || argString(args, ["command"]) || stripToolTitle(tool.parsed.title, "Bash") || "command"
    const exitCode = numberValue(metadata?.exit_code)
    return {
      label: state === "active" ? "Running command" : state === "error" ? "Command failed" : "Ran command",
      description: command,
      state,
      icon: state === "error" ? "warning" : "terminal",
      details: [
        {
          label: exitCode === undefined ? "Shell" : `Exit ${exitCode}`,
          detail: command,
          status: state,
        },
      ],
      diffLines: [],
      preview: tool.parsed.outputPreview,
    }
  }

  return {
    label:
      state === "active"
        ? `Running ${formatToolName(tool.parsed.name)}`
        : state === "error"
          ? `${formatToolName(tool.parsed.name)} failed`
          : formatToolName(tool.parsed.title || tool.parsed.name),
    description: firstPreviewLine(tool.parsed.outputPreview) || "Tool call",
    state,
    icon: state === "error" ? "warning" : "terminal",
    details: genericToolDetails(tool),
    diffLines: [],
    preview: tool.parsed.outputPreview,
  }
}

function planStepDetails(tool: ToolMessage): ToolStepDetail[] {
  const metadata = asObject(tool.parsed.metadata)
  const args = asObject(tool.parsed.args)
  const value = metadata?.plan ?? metadata?.todo_items ?? args?.plan ?? args?.todos
  const steps = arrayValue(value)
    .map((item): ToolStepDetail | null => {
      if (typeof item === "string") return { label: item.trim(), status: "complete" as ToolVisualState }
      const obj = asObject(item)
      const label = stringValue(obj?.step) || stringValue(obj?.content) || stringValue(obj?.title) || stringValue(obj?.description)
      if (!label) return null
      const rawStatus = stringValue(obj?.status)
      const status: ToolVisualState | undefined =
        rawStatus === "completed" || rawStatus === "complete" || rawStatus === "done"
          ? "complete"
          : rawStatus === "in_progress" || rawStatus === "active"
            ? "active"
            : undefined
      return {
        label,
        status,
      }
    })
    .filter((item): item is ToolStepDetail => item !== null && item.label.length > 0)

  if (steps.length > 0) return steps.slice(0, 8)

  return firstPreviewLine(tool.parsed.outputPreview)
    ? [{ label: firstPreviewLine(tool.parsed.outputPreview) ?? "Plan updated", status: toolState(tool) }]
    : []
}

function questionDetails(tool: ToolMessage): ToolStepDetail[] {
  const metadata = asObject(tool.parsed.metadata)
  const args = asObject(tool.parsed.args)
  const questions = arrayValue(metadata?.questions ?? args?.questions)
  return questions
    .map((question, index) => {
      const obj = asObject(question)
      const label =
        stringValue(obj?.question) ||
        stringValue(obj?.prompt) ||
        stringValue(obj?.header) ||
        (typeof question === "string" ? question : `Question ${index + 1}`)
      return { label, status: toolState(tool) }
    })
    .slice(0, 6)
}

function genericToolDetails(tool: ToolMessage): ToolStepDetail[] {
  const details: ToolStepDetail[] = []
  if (tool.parsed.title) details.push({ label: tool.parsed.title, status: toolState(tool) })
  const argsPreview = tool.parsed.args ? jsonSummary(tool.parsed.args) : ""
  if (argsPreview) details.push({ label: "Input", detail: argsPreview, status: toolState(tool) })
  return details
}

function combinedToolState(tools: ToolMessage[]): ToolVisualState {
  if (tools.some((tool) => toolState(tool) === "error")) return "error"
  if (tools.some((tool) => toolState(tool) === "active")) return "active"
  return "complete"
}

function toolState(tool: ToolMessage): ToolVisualState {
  const status = tool.parsed.status.toLowerCase()
  if (status === "error" || status === "failed") return "error"
  if (status === "running" || status === "pending") return "active"
  return "complete"
}

function asObject(value: JsonValue | undefined): JsonObject | undefined {
  if (value && typeof value === "object" && !Array.isArray(value)) return value
  return undefined
}

function arrayValue(value: JsonValue | undefined): JsonValue[] {
  return Array.isArray(value) ? value : []
}

function stringValue(value: JsonValue | undefined): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined
}

function numberValue(value: JsonValue | undefined): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined
}

function argString(obj: JsonObject | undefined, keys: string[]) {
  for (const key of keys) {
    const value = stringValue(obj?.[key])
    if (value) return value
  }
  return undefined
}

function stripToolTitle(title: string | undefined, label: string) {
  const prefix = `${label}:`
  return title?.startsWith(prefix) ? title.slice(prefix.length).trim() || undefined : undefined
}

function displayPath(raw: string, cwd: string, basenameOnly: boolean) {
  const trimmed = raw.trim() || "."
  if (basenameOnly) return basename(trimmed)
  if (cwd && trimmed === cwd) return "."
  if (cwd && trimmed.startsWith(`${cwd}/`)) return trimmed.slice(cwd.length + 1)
  return trimmed.replace(/^file:\/\//, "")
}

function readableUrl(raw: string) {
  try {
    const url = new URL(raw)
    return url.hostname.replace(/^www\./, "") + url.pathname.replace(/\/$/, "")
  } catch {
    return raw
  }
}

function firstPreviewLine(preview: string | undefined) {
  return preview
    ?.split("\n")
    .map((line) => line.trim())
    .find(Boolean)
}

function formatCount(count: number, noun: string, verb?: string) {
  const label = `${count} ${noun}${count === 1 ? "" : "s"}`
  return verb ? `${verb} ${label}` : label
}

function formatToolName(value: string) {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase())
}

function trimPreview(preview: string, maxChars = 1200) {
  const trimmed = preview.trim()
  return trimmed.length > maxChars ? `${trimmed.slice(0, maxChars).trimEnd()}\n...` : trimmed
}

function jsonSummary(value: JsonValue) {
  try {
    return trimPreview(JSON.stringify(value, null, 2), 420)
  } catch {
    return ""
  }
}

function lineNumberDetail(metadata: JsonObject | undefined, preview: string | undefined) {
  const line = numberValue(metadata?.line_number) ?? numberValue(metadata?.line) ?? numberValue(metadata?.start_line)
  if (line) return `line ${line}`
  return firstPreviewLine(preview)
}

function splitLines(text: string) {
  if (!text) return []
  const normalized = text.endsWith("\n") ? text.slice(0, -1) : text
  return normalized ? normalized.split("\n") : []
}

function diffStats(oldText: string, newText: string) {
  const oldLines = splitLines(oldText)
  const newLines = splitLines(newText)
  const lcs = lcsLength(oldLines, newLines)
  return {
    added: Math.max(0, newLines.length - lcs),
    removed: Math.max(0, oldLines.length - lcs),
  }
}

function diffLineOps(oldText: string, newText: string): DiffLine[] {
  const oldLines = splitLines(oldText)
  const newLines = splitLines(newText)

  if (oldLines.length === 0) return newLines.map((text) => ({ kind: "add", text }))
  if (newLines.length === 0) return oldLines.map((text) => ({ kind: "remove", text }))
  if (oldLines.length * newLines.length > 20000) {
    return [
      ...oldLines.slice(0, 4).map((text) => ({ kind: "remove" as const, text })),
      ...newLines.slice(0, 4).map((text) => ({ kind: "add" as const, text })),
    ]
  }

  const dp = lcsMatrix(oldLines, newLines)
  const ops: DiffLine[] = []
  let i = 0
  let j = 0

  while (i < oldLines.length && j < newLines.length) {
    if (oldLines[i] === newLines[j]) {
      ops.push({ kind: "context", text: oldLines[i] })
      i += 1
      j += 1
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push({ kind: "remove", text: oldLines[i] })
      i += 1
    } else {
      ops.push({ kind: "add", text: newLines[j] })
      j += 1
    }
  }

  while (i < oldLines.length) ops.push({ kind: "remove", text: oldLines[i++] })
  while (j < newLines.length) ops.push({ kind: "add", text: newLines[j++] })
  return ops
}

function compactDiffLines(lines: DiffLine[], maxLines = 12) {
  const changed = lines
    .map((line, index) => (line.kind === "context" ? -1 : index))
    .filter((index) => index >= 0)

  if (changed.length === 0) return lines.slice(0, Math.min(lines.length, maxLines))

  const start = Math.max(0, changed[0] - 2)
  const end = Math.min(lines.length, changed[changed.length - 1] + 3)
  return lines.slice(start, end).slice(0, maxLines)
}

function lcsLength(left: string[], right: string[]) {
  if (left.length * right.length > 20000) return 0
  const dp = lcsMatrix(left, right)
  return dp[0][0]
}

function lcsMatrix(left: string[], right: string[]) {
  const dp = Array.from({ length: left.length + 1 }, () => Array(right.length + 1).fill(0))
  for (let i = left.length - 1; i >= 0; i -= 1) {
    for (let j = right.length - 1; j >= 0; j -= 1) {
      dp[i][j] = left[i] === right[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
    }
  }
  return dp
}

function patchPaths(patch: string, cwd: string) {
  const paths = new Set<string>()
  for (const line of patch.split("\n")) {
    const codexPath =
      line.match(/^\*\*\* (?:Update|Add|Delete) File: (.+)$/)?.[1] ||
      line.match(/^\+\+\+ b\/(.+)$/)?.[1] ||
      line.match(/^--- a\/(.+)$/)?.[1]
    if (codexPath && codexPath !== "/dev/null") paths.add(displayPath(codexPath, cwd, false))
  }
  return [...paths]
}

function ModelList(props: {
  models: RemoteModel[]
  activeIndex: number
  onActiveIndex: (index: number) => void
  onSelect: (model: RemoteModel) => void
}) {
  let group = ""
  return (
    <For each={props.models}>
      {(model, index) => {
        const showGroup = model.group !== group
        group = model.group
        return (
          <>
            <Show when={showGroup}>
              <div class="px-2 pt-3 pb-1 text-[0.66rem] font-bold uppercase tracking-[0.07em] text-[var(--faint)]">
                {model.group || "Models"}
              </div>
            </Show>
            <button
              class={cx(
                "flex min-h-[2.7rem] w-full items-center justify-between gap-3 rounded-lg px-2 py-2 text-left text-[var(--text)] transition hover:bg-[#2b2b2b]",
                props.activeIndex === index() && MENU_ROW_ACTIVE
              )}
              type="button"
              role="option"
              aria-selected={props.activeIndex === index()}
              onMouseEnter={() => props.onActiveIndex(index())}
              onClick={() => props.onSelect(model)}
            >
              <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                <span class="flex min-w-0 items-center gap-2">
                  <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.86rem] font-semibold">
                    {model.name || model.id}
                  </span>
                  <Show when={model.active}>
                    <span class="shrink-0 rounded-full border border-[rgba(92,168,134,0.35)] px-1.5 py-0.5 text-[0.64rem] font-bold text-[var(--green)]">
                      Active
                    </span>
                  </Show>
                </span>
                <span class="block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-[0.72rem] text-[var(--faint)]">
                  {providerLabel(model)}
                </span>
              </span>
            </button>
          </>
        )
      }}
    </For>
  )
}

function SuggestionIcon(props: { suggestion: RemoteSuggestion }) {
  const suggestion = props.suggestion
  if (suggestion.kind === "command") return <IconTerminal class="h-[1.35rem] w-[1.35rem] text-[var(--muted)]" />
  if (suggestion.kind === "agent") return <IconBrainGlyph class="h-[1.35rem] w-[1.35rem] text-[#d9a6ff]" />
  if (suggestion.is_directory) return <IconFolder class="h-[1.35rem] w-[1.35rem] text-[#85827a]" />

  const FileIcon = iconForFile(suggestion.name)
  return <FileIcon class="h-[1.35rem] w-[1.35rem]" />
}

function iconForFile(path: string) {
  const lower = path.toLowerCase()
  if (lower.endsWith(".rs")) return IconIconFileRust
  if (lower.endsWith(".tsx")) return IconIconFileTsx
  if (lower.endsWith(".ts")) return IconIconFileTs
  if (lower.endsWith(".jsx") || lower.endsWith(".js")) return IconIconFileJs
  if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return IconIconFileJson
  if (lower.endsWith(".md") || lower.endsWith(".mdx")) return IconIconFileMarkdown
  if (lower.endsWith(".toml")) return IconIconFileToml
  if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return IconIconFileYaml
  if (lower.endsWith(".css")) return IconIconFileCss
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return IconIconFileHtml
  return IconIconFileDefault
}

function suggestionPrefix(suggestion: RemoteSuggestion) {
  if (suggestion.kind === "command") return "/"
  if (suggestion.kind === "agent") return "@"
  return ""
}

function EmptyThread(props: { projectName: string; mascotFrame: string }) {
  return (
    <div class="grid min-h-0 min-w-0 place-items-center overflow-hidden text-center text-[var(--faint)]">
      <div class="grid max-w-full justify-items-center gap-4 overflow-hidden" aria-label={props.projectName}>
        <pre class="m-0 whitespace-pre font-mono text-[clamp(0.58rem,2.1vw,1.08rem)] font-bold leading-none tracking-normal text-[var(--brand-primary)] [font-variant-ligatures:none]">
          {props.mascotFrame}
        </pre>
        <pre class="m-0 whitespace-pre bg-[linear-gradient(180deg,var(--brand-primary)_0_63%,var(--brand-dim)_63%_100%)] bg-clip-text font-mono text-[clamp(0.52rem,2.55vw,1.22rem)] font-bold leading-none tracking-normal text-transparent [font-variant-ligatures:none]">
          {LOGO_ART}
        </pre>
      </div>
    </div>
  )
}

function relativeTime(seconds: number) {
  if (!seconds) return ""
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - seconds)
  if (diff < 60) return "now"
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`
  return new Date(seconds * 1000).toLocaleDateString()
}

function basename(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? ""
}

function normalizeArt(source: string, options: { trimCommonIndent?: boolean } = {}) {
  let lines = source.trimEnd().split("\n")
  if (options.trimCommonIndent) {
    const indents = lines
      .filter((line) => line.trim().length > 0)
      .map((line) => line.match(/^ */)?.[0].length ?? 0)
    const indent = Math.min(...indents)
    if (Number.isFinite(indent) && indent > 0) {
      lines = lines.map((line) => line.slice(indent))
    }
  }

  const width = Math.max(0, ...lines.map((line) => line.length))
  return lines.map((line) => line.padEnd(width, " ")).join("\n")
}

function loadSavedServers(): SavedServer[] {
  try {
    const raw = localStorage.getItem(SERVERS_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as SavedServer[]
    if (!Array.isArray(parsed)) return []
    return parsed.filter((server) => normalizeServerAddress(server.address))
  } catch {
    return []
  }
}

function saveSavedServers(servers: SavedServer[]) {
  localStorage.setItem(SERVERS_KEY, JSON.stringify(servers))
}

function normalizeServerAddress(address: string) {
  const value = address.trim()
  if (!value) return ""
  const withProtocol = /^https?:\/\//i.test(value) ? value : `http://${value}`
  try {
    const url = new URL(withProtocol)
    url.pathname = url.pathname === "/" ? "" : url.pathname.replace(/\/+$/, "")
    url.search = ""
    url.hash = ""
    return url.toString().replace(/\/$/, "")
  } catch {
    return withProtocol.replace(/\/+$/, "")
  }
}

function browserOrigin() {
  return typeof window === "undefined" ? "" : window.location.origin
}

function isActiveServer(address: string, activeAddress: string) {
  return normalizeServerAddress(address) === normalizeServerAddress(activeAddress)
}

function sameToken(left: string, right: string) {
  return left.trim().toLowerCase() === right.trim().toLowerCase()
}

function showErrorToast(error: unknown, fallback: string) {
  toast.error(errorToastMessage(error, fallback))
}

function errorToastMessage(error: unknown, fallback: string) {
  if (error instanceof Error && error.message.trim()) return error.message
  return fallback
}

function loadPromptHistory() {
  try {
    const parsed = JSON.parse(localStorage.getItem(PROMPT_HISTORY_KEY) || "[]") as unknown
    if (!Array.isArray(parsed)) return []
    return mergePromptHistoryEntries(parsed.filter((item): item is string => typeof item === "string"))
  } catch {
    return []
  }
}

function savePromptHistory(entries: string[]) {
  try {
    localStorage.setItem(PROMPT_HISTORY_KEY, JSON.stringify(entries.slice(0, MAX_PROMPT_HISTORY)))
  } catch {
    // Losing local browser history should not block chat input.
  }
}

function messagePromptHistoryEntries(messages: RemoteMessage[]) {
  return messages
    .filter((message) => message.role === "user")
    .map((message) => message.content)
    .reverse()
}

function mergePromptHistoryEntries(...groups: string[][]) {
  const seen = new Set<string>()
  const entries: string[] = []

  for (const text of groups.flat()) {
    const entry = normalizePromptHistoryEntry(text)
    if (!entry || seen.has(entry) || parseSlashCommand(entry)) continue
    seen.add(entry)
    entries.push(entry)
    if (entries.length >= MAX_PROMPT_HISTORY) break
  }

  return entries
}

function normalizePromptHistoryEntry(text: string) {
  return text.trim()
}

function isCursorOnFirstLogicalLine(text: string, cursor: number) {
  return !text.slice(0, Math.max(0, cursor)).includes("\n")
}

function isCursorOnLastLogicalLine(text: string, cursor: number) {
  return !text.slice(Math.max(0, cursor)).includes("\n")
}

function displayAgentMode(agent: string) {
  const normalized = agent.trim() || "Build"
  return normalized
    .split(/([\s_-]+)/)
    .map((part) => (/^[\s_-]+$/.test(part) ? part : part.charAt(0).toUpperCase() + part.slice(1)))
    .join("")
}

function agentAccentClass(agent: string) {
  if (sameToken(agent, "Plan")) return "border-[#8fcfb1] text-[#8fcfb1]"
  return "border-[#bda0ff] text-[#bda0ff]"
}

function isSupportedImageFile(file: File) {
  return IMAGE_FILE_TYPES.includes(file.type)
}

function readComposerAttachment(file: File): Promise<ComposerAttachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      if (typeof reader.result !== "string" || !reader.result.startsWith("data:image/")) {
        reject(new Error("Could not read image."))
        return
      }

      resolve({
        id: cuid(),
        name: file.name || "pasted-image.png",
        mediaType: file.type || mediaTypeFromDataUrl(reader.result),
        size: file.size,
        dataUrl: reader.result,
      })
    }
    reader.onerror = () => reject(new Error("Could not read image."))
    reader.readAsDataURL(file)
  })
}

function filesFromClipboard(clipboardData: DataTransfer | null) {
  if (!clipboardData) return []

  const files = Array.from(clipboardData.files ?? []).filter((file) => file.type.startsWith("image/"))
  if (files.length > 0) return files

  return Array.from(clipboardData.items ?? [])
    .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file))
}

function promptTextWithAttachmentPlaceholders(rawText: string, attachmentCount: number) {
  if (attachmentCount <= 0) return rawText

  let text = rawText
  for (let index = 1; index <= attachmentCount; index += 1) {
    const placeholder = `[Image #${index}]`
    if (!text.includes(placeholder)) {
      if (text.length > 0 && !/\s$/.test(text)) text += " "
      text += placeholder
    }
  }
  return text
}

function promptTextParts(text: string, attachmentCount: number): PromptTextPart[] {
  const parts: PromptTextPart[] = []
  let cursor = 0
  const ranges = [
    ...imagePlaceholderRanges(text)
      .filter((range) => range.number >= 1 && range.number <= attachmentCount)
      .map((range) => ({ kind: "image" as const, start: range.start, end: range.end })),
    ...agentMentionRanges(text).map((range) => ({ kind: "mention" as const, start: range.start, end: range.end })),
  ].sort((left, right) => left.start - right.start || left.end - right.end)

  for (const range of ranges) {
    if (range.start < cursor) continue
    if (range.start > cursor) {
      parts.push({ kind: "text", text: text.slice(cursor, range.start) })
    }
    parts.push({ kind: range.kind, text: text.slice(range.start, range.end) })
    cursor = range.end
  }

  if (cursor < text.length) {
    parts.push({ kind: "text", text: text.slice(cursor) })
  }

  return parts
}

function agentMentionRanges(text: string): Array<{ start: number; end: number }> {
  return Array.from(text.matchAll(/(^|[\s([{])(@[A-Za-z0-9][A-Za-z0-9_-]*)/g)).map((match) => {
    const prefixLength = match[1]?.length ?? 0
    const start = (match.index ?? 0) + prefixLength
    return {
      start,
      end: start + match[2].length,
    }
  })
}

function promptTextPartClass(part: PromptTextPart) {
  if (part.kind === "image") {
    return "rounded-[4px] bg-[rgba(118,185,145,0.14)] text-[#9ed8b7] shadow-[0_0_0_1px_rgba(118,185,145,0.18)]"
  }
  if (part.kind === "mention") {
    return "rounded-[4px]"
  }
  return undefined
}

function promptTextPartStyle(part: PromptTextPart): JSX.CSSProperties | undefined {
  if (part.kind !== "mention") return undefined
  const accent = mentionAccent(part.text)
  return {
    color: accent.text,
    "background-color": accent.background,
    "box-shadow": `0 0 0 1px ${accent.ring}`,
  } as JSX.CSSProperties
}

function mentionAccent(text: string) {
  const key = text.replace(/^@/, "").toLowerCase()
  let hash = 0
  for (let index = 0; index < key.length; index += 1) {
    hash = (hash * 31 + key.charCodeAt(index)) >>> 0
  }
  return MENTION_ACCENTS[hash % MENTION_ACCENTS.length]
}

function imagePlaceholderRanges(text: string): ImagePlaceholderRange[] {
  return Array.from(text.matchAll(/\[Image #(\d+)\]/g)).map((match) => ({
    number: Number(match[1]),
    start: match.index ?? 0,
    end: (match.index ?? 0) + match[0].length,
  }))
}

function rangesIntersect(leftStart: number, leftEnd: number, rightStart: number, rightEnd: number) {
  return leftStart < rightEnd && rightStart < leftEnd
}

function removeRangesFromText(text: string, ranges: Array<{ start: number; end: number }>) {
  if (ranges.length === 0) return text

  const sorted = [...ranges]
    .filter((range) => range.end > range.start)
    .sort((left, right) => left.start - right.start || left.end - right.end)
  const merged: Array<{ start: number; end: number }> = []

  for (const range of sorted) {
    const last = merged[merged.length - 1]
    if (last && range.start <= last.end) {
      last.end = Math.max(last.end, range.end)
    } else {
      merged.push({ ...range })
    }
  }

  let output = ""
  let cursor = 0
  for (const range of merged) {
    output += text.slice(cursor, range.start)
    cursor = range.end
  }
  output += text.slice(cursor)
  return output
}

function renumberImagePlaceholdersAfterRemoval(
  text: string,
  removedNumbers: number[],
  attachmentCount: number
) {
  const removed = new Set(removedNumbers)
  return text
    .replace(/\[Image #(\d+)\]/g, (placeholder, rawNumber) => {
      const number = Number(rawNumber)
      if (!Number.isFinite(number)) return placeholder
      if (number < 1 || number > attachmentCount) return placeholder
      if (removed.has(number)) return ""
      const offset = removedNumbers.filter((removedNumber) => removedNumber < number).length
      if (offset > 0) return `[Image #${number - offset}]`
      return placeholder
    })
    .replace(/[ \t]{2,}/g, " ")
    .replace(/[ \t]+\n/g, "\n")
    .trimStart()
}

function imagePreviewFromAttachment(attachment: AttachmentData): ImagePreviewTarget | null {
  const mediaType = attachment.mediaType?.toLowerCase() ?? ""
  if (mediaType && !mediaType.startsWith("image/")) return null
  return {
    url: attachment.url,
    label: attachment.filename?.trim() || "Image attachment",
  }
}

function handleImagePreviewKeyDown(event: KeyboardEvent, onOpen: () => void) {
  if (event.key !== "Enter" && event.key !== " ") return
  event.preventDefault()
  onOpen()
}

function messageImageAttachmentData(message: RemoteMessage, token: string): AttachmentData[] {
  return (message.local_image_paths ?? []).map((path, index) => ({
    id: `${index}-${path}`,
    url: localImageUrl(path, token),
    filename: `[Image #${index + 1}] ${basename(path) || "image"}`,
    mediaType: imageMediaTypeFromPath(path),
  }))
}

function localImageUrl(path: string, token: string) {
  const url = new URL("/api/local-image", window.location.origin)
  url.searchParams.set("path", path)
  if (token) url.searchParams.set("token", token)
  return url.toString()
}

function imageMediaTypeFromPath(path: string) {
  const extension = path.split("?")[0]?.split(".").pop()?.toLowerCase()
  if (extension === "jpg" || extension === "jpeg") return "image/jpeg"
  if (extension === "gif") return "image/gif"
  if (extension === "webp") return "image/webp"
  return "image/png"
}

function mediaTypeFromDataUrl(dataUrl: string) {
  return dataUrl.slice(5, dataUrl.indexOf(";")) || "image/png"
}

function messageModelLabel(message: RemoteMessage, status: RemoteStatus | null) {
  const model = message.model || status?.model || "model"
  const provider = message.provider || status?.provider || ""
  if (!provider) return model
  if (model.startsWith(`${provider}/`)) return model
  return `${provider}/${model}`
}

function assistantMetrics(message: RemoteMessage) {
  if (!message.is_complete) return []
  const metrics: string[] = []

  if (message.t0_ms != null && message.t1_ms != null && message.tn_ms != null) {
    const totalMs = Math.max(0, message.tn_ms - message.t0_ms)
    const ttftMs = Math.max(0, message.t1_ms - message.t0_ms)
    const decodeMs = Math.max(0, message.tn_ms - message.t1_ms)
    const tokens = message.output_tokens ?? message.token_count ?? 0
    metrics.push(formatSeconds(totalMs))
    metrics.push(`ttft ${formatSeconds(ttftMs)}`)
    if (decodeMs > 0 && tokens > 0) metrics.push(`${Math.round(tokens / (decodeMs / 1000))}t/s`)
  } else if (message.token_count != null && message.duration_ms != null) {
    metrics.push(formatSeconds(message.duration_ms))
    if (message.duration_ms > 0) {
      metrics.push(`${Math.round(message.token_count / (message.duration_ms / 1000))}t/s`)
    }
  }

  if (message.was_interrupted) metrics.push("interrupted")
  return metrics
}

function formatSeconds(ms: number) {
  return `${(ms / 1000).toFixed(1)}s`
}

function fallbackCopyText(text: string) {
  const textarea = document.createElement("textarea")
  textarea.value = text
  textarea.setAttribute("readonly", "")
  textarea.style.position = "fixed"
  textarea.style.opacity = "0"
  textarea.style.pointerEvents = "none"
  document.body.appendChild(textarea)
  textarea.select()
  document.execCommand("copy")
  document.body.removeChild(textarea)
}

function CountBadge(props: { count: number }) {
  return (
    <span class="rounded-md bg-white/[0.075] px-1.5 py-0.5 text-[0.68rem] font-bold leading-none text-[var(--text)]">
      {props.count}
    </span>
  )
}

function handleChoiceMenuKeyDown(
  event: KeyboardEvent,
  open: boolean,
  setOpen: Setter<boolean>,
  options: string[],
  activeIndex: number,
  setActiveIndex: Setter<number>,
  onSelect: (value: string) => void | Promise<void>,
  onEscape?: () => void
) {
  if (options.length === 0) return

  if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault()
    setOpen(true)
    setActiveIndex(event.key === "ArrowUp" ? options.length - 1 : Math.max(activeIndex, 0))
    return
  }

  if (!open) return

  if (event.key === "ArrowDown") {
    event.preventDefault()
    setActiveIndex((index) => (index + 1) % options.length)
    return
  }

  if (event.key === "ArrowUp") {
    event.preventDefault()
    setActiveIndex((index) => (index - 1 + options.length) % options.length)
    return
  }

  if (event.key === "Enter") {
    event.preventDefault()
    const selected = options[activeIndex]
    if (selected) void onSelect(selected)
    return
  }

  if (event.key === "Escape") {
    event.preventDefault()
    onEscape?.()
    setOpen(false)
  }
}

function useStickToBottom(
  scrollEl: Accessor<HTMLElement | undefined>,
  contentEl: Accessor<HTMLElement | undefined>
) {
  const [isAtTop, setIsAtTop] = createSignal(true)
  const [isAtBottom, setIsAtBottom] = createSignal(true)
  const bottomThreshold = 32
  const topThreshold = 8

  const measure = () => {
    const el = scrollEl()
    if (!el) return
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight
    setIsAtBottom(distance <= bottomThreshold)
    setIsAtTop(el.scrollTop <= topThreshold)
  }

  const scrollToBottom = (smooth = false) => {
    const el = scrollEl()
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" })
    measure()
  }

  onMount(() => {
    queueMicrotask(() => scrollToBottom(false))
  })

  createEffect(() => {
    const el = scrollEl()
    const content = contentEl()
    if (!el) return
    measure()
    el.addEventListener("scroll", measure, { passive: true })

    const resizeObserver = new ResizeObserver(() => {
      if (isAtBottom()) scrollToBottom(false)
      else measure()
    })
    resizeObserver.observe(content ?? el)

    onCleanup(() => {
      el.removeEventListener("scroll", measure)
      resizeObserver.disconnect()
    })
  })

  return { isAtTop, isAtBottom, scrollToBottom, measure }
}

function cuid() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID()
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`
}

function providerLabel(model: RemoteModel) {
  const raw = model.description || model.provider_id
  return raw.split("|")[0].trim() || model.provider_id
}

function detectCompletionTrigger(text: string, cursor: number): CompletionTrigger | null {
  const safeCursor = Math.max(0, Math.min(cursor, text.length))
  const beforeCursor = text.slice(0, safeCursor)

  if (beforeCursor.startsWith("/") && !beforeCursor.includes("\n")) {
    const query = beforeCursor.slice(1)
    if (!query.includes(" ")) return { kind: "slash", query, range: [0, safeCursor] }
  }

  const atIndex = beforeCursor.lastIndexOf("@")
  if (atIndex < 0) return null
  if (atIndex > 0 && !/\s/.test(beforeCursor[atIndex - 1])) return null

  const query = beforeCursor.slice(atIndex + 1)
  if (/\s/.test(query)) return null
  const afterCursor = text.slice(safeCursor)
  const afterToken = afterCursor.search(/\s/)
  const end = afterToken < 0 ? text.length : safeCursor + afterToken
  return { kind: "mention", query, range: [atIndex, end] }
}

function quoteCompletionPath(path: string) {
  return /\s/.test(path) ? `"${path.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"` : path
}

function parseSlashCommand(text: string) {
  const trimmed = text.trim()
  if (!trimmed.startsWith("/")) return null
  const body = trimmed.slice(1).trimStart()
  const match = body.match(/^([^\s]+)(?:\s+([\s\S]*))?$/)
  if (!match) return null
  return { name: match[1], args: match[2] ?? "" }
}

function sessionTranscript(title: string, messages: RemoteMessage[]) {
  const parts = [`# ${title || "Untitled"}`]

  for (const message of messages) {
    if (message.role === "system") continue
    if (message.role === "user") {
      parts.push(`## User\n\n${message.content}`)
      continue
    }
    if (message.role === "assistant") {
      const agent = message.agent_mode || "Build"
      const model = message.model || "unknown"
      parts.push(`## Assistant (${agent} · ${model})\n\n${message.content}`)
      continue
    }
    if (message.role === "tool") {
      parts.push(`**Tool Result**\n\n${formatToolTranscript(message.content)}`)
    }
  }

  return `${parts.join("\n\n---\n\n")}\n`
}

function formatToolTranscript(content: string) {
  try {
    const value = JSON.parse(content) as JsonValue
    return `\`\`\`json\n${JSON.stringify(value, null, 2)}\n\`\`\``
  } catch {
    return `\`\`\`\n${content}\n\`\`\``
  }
}
