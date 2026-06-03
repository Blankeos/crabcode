import { useHotkeys } from "bagon-hooks"
import { createEffect, createMemo, createSignal, type JSX, onCleanup, onMount } from "solid-js"
import { toast } from "solid-sonner"
import { type AttachmentData } from "../../components/ai-elements/attachments"
import { cx } from "../../lib/cx"
import {
  createRemoteApi,
  RemoteApiError,
  type RemoteModel,
  type RemotePromptImage,
  type RemoteSkill,
  type RemoteState,
  type RemoteSuggestion,
} from "../../remote-api"
import "../../styles/app.css"
import { MASCOT_FRAMES } from "./ascii-art"
import { RemoteClientPage } from "./page-layout"
import { AGENT_MODES, MAX_COMPOSER_ATTACHMENT_BYTES, MAX_COMPOSER_ATTACHMENTS, MAX_PROMPT_HISTORY } from "./page-constants"
import type { CompletionTrigger, ComposerAttachment, ImagePreviewTarget, ProjectPathFormController, RemoteClientUi, RemotePermissionResponse, SavedServer, ServerPanelController, ServerPanelTab } from "./page-types"
import {
  detectCompletionTrigger,
  filesFromClipboard,
  imagePlaceholderRanges,
  imagePreviewFromAttachment,
  isCursorOnFirstPromptLine,
  isCursorOnLastPromptLine,
  isSupportedImageFile,
  loadPromptHistory,
  mergePromptHistoryEntries,
  messagePromptHistoryEntries,
  normalizePromptHistoryEntry,
  parseSlashCommand,
  promptTextWithAttachmentPlaceholders,
  quoteCompletionPath,
  rangesIntersect,
  readComposerAttachment,
  removeRangesFromText,
  renumberImagePlaceholdersAfterRemoval,
  savePromptHistory,
} from "./prompt-utils"
import { projectsFromState } from "./projects"
import { browserOrigin, isActiveServer, loadSavedServers, normalizeServerAddress, saveSavedServers } from "./server-utils"
import { basename, errorToastMessage, fallbackCopyText, handleChoiceMenuKeyDown, sameToken, showErrorToast, useStickToBottom, cuid } from "./shared-utils"
import { buildThreadItems, sessionTranscript } from "./thread-model"

const TOKEN_KEY = "crabcode.remote.token"

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
      if (!isCursorOnFirstPromptLine(event.currentTarget, text, cursor)) return false

      const currentIndex = promptHistoryIndex()
      const nextIndex = currentIndex == null ? 0 : Math.min(currentIndex + 1, entries.length - 1)
      if (currentIndex === nextIndex) return false

      event.preventDefault()
      if (currentIndex == null) setPromptHistoryDraft(text)
      setPromptHistoryIndex(nextIndex)
      applyPromptHistoryEntry(entries[nextIndex] ?? "", "start")
      return true
    }

    if (!isCursorOnLastPromptLine(event.currentTarget, text, cursor)) return false

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

  const projectPathForm: ProjectPathFormController = {
    value: projectPathInput,
    setValue: setProjectPathInput,
    error: projectPathError,
    setError: setProjectPathError,
    setInputRef: (element) => {
      projectPathInputRef = element
    },
    focusInput: () => queueMicrotask(() => projectPathInputRef?.focus()),
    onSubmit: submitProjectPath,
  }

  const serversController: ServerPanelController = {
    popoverOpen: serversOpen,
    onPopoverOpenChange: handleServersOpenChange,
    manageOpen: serversManageOpen,
    setManageOpen: setServersManageOpen,
    addOpen: serverAddOpen,
    setAddOpen: setServerAddOpen,
    tab: serverPanelTab,
    onSelectTab: selectServerPanelTab,
    search: serverSearch,
    setSearch: setServerSearch,
    address: serverAddress,
    setAddress: setServerAddress,
    name: serverName,
    setName: setServerName,
    username: serverUsername,
    setUsername: setServerUsername,
    password: serverPassword,
    setPassword: setServerPassword,
    setAddressRef: (element) => {
      serverAddressRef = element
    },
    servers,
    filteredServers,
    skills,
    activeServerUrl,
    status: () => state()?.status ?? null,
    onOpenManager: openServerManager,
    onShowAddServer: showAddServer,
    onSaveServer: saveServer,
    onOpenServer: openServer,
  }

  const ui: RemoteClientUi = {
    themeStyle,
    pair: {
      required: pairRequired,
      code: pairCode,
      setCode: setPairCode,
      error: pairError,
      onSubmit: pair,
    },
    sidebar: {
      open: sidebarOpen,
      setOpen: setSidebarOpen,
      onOpenCommandPalette: openCommandPalette,
      newProjectOpen,
      onNewProjectOpenChange: handleNewProjectOpenChange,
      projectPathForm,
      projects,
      openProjects: projectOpen,
      activeProjectPath: projectPath,
      token,
      currentSessionId: () => state()?.current_session_id,
      onToggleProject: toggleProject,
      onNewSession: startNewSession,
      onSwitchSession: switchSession,
      onArchiveSession: archiveSession,
      onArchiveProject: archiveProject,
    },
    header: {
      setSidebarOpen,
      projectPicker: {
        open: projectPickerOpen,
        addOpen: projectPickerAddOpen,
        setAddOpen: setProjectPickerAddOpen,
        onOpenChange: handleProjectPickerOpenChange,
        projectName,
        projectPath,
        projects,
        token,
        form: projectPathForm,
        onSelectWorkspace: selectWorkspace,
      },
      isEmptyChat,
      onNewSession: startNewSession,
      servers: serversController,
    },
    thread: {
      setScrollRef: (element) => setThreadScrollEl(element),
      setContentRef: (element) => setThreadContentEl(element),
      isAtTop: threadScroll.isAtTop,
      isAtBottom: threadScroll.isAtBottom,
      isEmptyChat,
      visibleMessages,
      threadItems,
      projectName,
      mascotFrame: () => MASCOT_FRAMES[mascotFrame()] ?? "",
      status: () => state()?.status ?? null,
      token,
      onPreviewImage: openImagePreview,
    },
    composer: {
      pendingPermission,
      permissionBusy,
      onAnswerPermission: answerPermission,
      pendingQuestion,
      questionBusy,
      onAnswerQuestion: answerQuestion,
      onCancelQuestion: cancelQuestion,
      onSubmit: submitPrompt,
      onDrop: handleComposerDrop,
      setImageInputRef: (element) => {
        imageInputRef = element
      },
      openImageInput: () => imageInputRef?.click(),
      onAddImageFiles: addImageFiles,
      attachments: composerAttachments,
      attachmentData: composerAttachmentData,
      onRemoveAttachment: removeComposerAttachment,
      onPreviewImage: openImagePreview,
      prompt,
      promptAttachmentCount: () => composerAttachments().length,
      setPromptRef: (element) => {
        promptRef = element
      },
      setPromptOverlayRef: (element) => {
        promptOverlayRef = element
      },
      onPromptInput: handlePromptInput,
      onPromptKeyDown: handlePromptKeyDown,
      onRefreshCompletion: refreshCompletion,
      onPromptScroll: handlePromptScroll,
      onPromptPaste: handlePromptPaste,
      suggestions: composerSuggestions,
      suggestionIndex: composerSuggestionIndex,
      setSuggestionIndex: setComposerSuggestionIndex,
      setSuggestionsRef: (element) => {
        composerSuggestionsRef = element
      },
      onChooseSuggestion: chooseComposerSuggestion,
      modelOpen,
      onModelOpenChange: handleModelOpenChange,
      modelLabel,
      setModelSearchRef: (element) => {
        modelSearchRef = element
      },
      modelQuery,
      setModelQuery,
      onModelSearchKeyDown: handleModelSearchKeyDown,
      filteredModels,
      modelActiveIndex,
      setModelActiveIndex,
      onSelectModel: selectModel,
      onControlPopoverCloseAutoFocus: handleControlPopoverCloseAutoFocus,
      onControlEscape: requestPromptFocusAfterControlPopoverClose,
      agentOpen,
      onAgentOpenChange: handleAgentOpenChange,
      onAgentKeyDown: handleAgentMenuKeyDown,
      agentActiveIndex,
      setAgentActiveIndex,
      onSelectAgentMode: selectAgentMode,
      reasoningOpen,
      onReasoningOpenChange: handleReasoningOpenChange,
      onReasoningKeyDown: handleReasoningMenuKeyDown,
      reasoningOptions,
      reasoningLabel,
      reasoningActiveIndex,
      setReasoningActiveIndex,
      onSelectReasoningEffort: selectReasoningEffort,
      status: () => state()?.status ?? null,
      streaming: () => Boolean(state()?.is_streaming),
    },
    commandPalette: {
      rendered: commandRendered,
      closing: commandClosing,
      query: commandQuery,
      setQuery: setCommandQuery,
      setInputRef: (element) => {
        commandInputRef = element
      },
      onClose: closeCommandPalette,
      isEmptyChat,
      onNewSession: startNewSession,
      projectResults: projectCommandResults,
      sessionResults: commandResults,
      onSwitchSession: switchSession,
    },
    servers: serversController,
    imagePreview,
    onCloseImagePreview: () => setImagePreview(null),
  }

  return <RemoteClientPage ui={ui} />
}
