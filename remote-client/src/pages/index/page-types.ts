import type { Accessor, JSX, Setter } from "solid-js"
import type { AttachmentData } from "../../components/ai-elements/attachments"
import type { ProjectGroup } from "../../components/remote/project-list"
import type {
  RemoteMessage,
  RemoteGitStatus,
  RemoteModel,
  RemotePendingPermission,
  RemotePendingQuestion,
  RemoteSkill,
  RemoteState,
  RemoteStatus,
  RemoteSuggestion,
  RemoteThreadTabs,
} from "../../remote-api"

export type SavedServer = {
  id: string
  address: string
  name: string
  username: string
  password: string
}

export type RemotePermissionResponse = "deny" | "allow_once" | "allow_always"

export type ServerPanelTab = "servers" | "skills" | "mcp" | "lsp" | "plugins"

export type CompletionTrigger = {
  kind: "slash" | "mention"
  query: string
  range: [number, number]
}

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
export type JsonObject = { [key: string]: JsonValue }

export type ParsedToolMessage = {
  id: string
  name: string
  status: string
  args?: JsonValue
  metadata?: JsonValue
  outputPreview?: string
  title?: string
  lineCount?: number
}

export type ToolMessage = {
  message: RemoteMessage
  parsed: ParsedToolMessage
  cwd: string
}

export type ThreadItem =
  | { type: "message"; message: RemoteMessage; activityTools: ToolMessage[] }
  | { type: "activity"; tools: ToolMessage[] }
  | { type: "action"; tool: ToolMessage }

export type AssistantSegment =
  | { kind: "text"; text: string }
  | { kind: "action"; tool: ToolMessage }
  | { kind: "activity"; tools: ToolMessage[] }

export type ComposerAttachment = {
  id: string
  name: string
  mediaType: string
  size: number
  dataUrl: string
}

export type ImagePreviewTarget = {
  url: string
  label: string
}

export type PromptTextPart = {
  kind: "text" | "image" | "mention"
  text: string
}

export type ImagePlaceholderRange = {
  number: number
  start: number
  end: number
}

export type ToolVisualState = "active" | "complete" | "error"
export type ToolIconKind = "agent" | "brain" | "check" | "file" | "globe" | "pencil" | "search" | "terminal" | "warning"

export type ToolStepDetail = {
  label: string
  detail?: string
  status?: ToolVisualState
}

export type ToolActivityStep = {
  key: string
  label: string
  icon: ToolIconKind
  state: ToolVisualState
  details: ToolStepDetail[]
  subagents?: SubagentActivityItem[]
  preview?: string
  defaultOpen?: boolean
}

export type SubagentActivityItem = {
  id: string
  agent: string
  description: string
  title?: string
  sessionId?: string
  durationMs?: number
  toolCallCount?: number
  state: ToolVisualState
  preview?: string
}

export type DiffLine = {
  kind: "add" | "remove" | "context"
  text: string
  lineNumber?: number
  language?: string
}

export type DiffSection = {
  path: string
  language?: string
  lines: DiffLine[]
}

export type ActionDescriptor = {
  label: string
  description: string
  state: ToolVisualState
  icon: ToolIconKind
  stats?: { added: number; removed: number }
  details: ToolStepDetail[]
  diffLines: DiffLine[]
  diffSections?: DiffSection[]
  preview?: string
}

export type MaybePromise = void | Promise<void>
export type RefSetter<T extends HTMLElement> = (element: T) => void

export type ProjectPathFormController = {
  value: Accessor<string>
  setValue: Setter<string>
  error: Accessor<string>
  setError: Setter<string>
  setInputRef: RefSetter<HTMLInputElement>
  focusInput: () => void
  onSubmit: (event: SubmitEvent) => MaybePromise
}

export type PairPanelController = {
  required: Accessor<boolean>
  code: Accessor<string>
  setCode: Setter<string>
  error: Accessor<string>
  onSubmit: (event: SubmitEvent) => MaybePromise
}

export type SidebarController = {
  open: Accessor<boolean>
  setOpen: Setter<boolean>
  onOpenCommandPalette: () => void
  newProjectOpen: Accessor<boolean>
  onNewProjectOpenChange: (open: boolean) => void
  projectPathForm: ProjectPathFormController
  projects: Accessor<ProjectGroup[]>
  openProjects: Accessor<Set<string>>
  allProjectsExpanded: Accessor<boolean>
  activeProjectPath: Accessor<string>
  token: Accessor<string>
  currentSessionId: Accessor<string | null | undefined>
  onToggleProject: (key: string) => void
  onToggleAllProjects: () => void
  onNewSession: (workspacePath?: string) => MaybePromise
  onSwitchSession: (id: string) => MaybePromise
  onArchiveSession: (id: string) => MaybePromise
  onArchiveProject: (path: string) => MaybePromise
}

export type ProjectPickerController = {
  open: Accessor<boolean>
  addOpen: Accessor<boolean>
  setAddOpen: Setter<boolean>
  onOpenChange: (open: boolean) => void
  projectName: Accessor<string>
  projectPath: Accessor<string>
  projects: Accessor<ProjectGroup[]>
  token: Accessor<string>
  form: ProjectPathFormController
  onSelectWorkspace: (path: string) => MaybePromise
  onResumeProject: (path: string) => MaybePromise
}

export type ServerPanelController = {
  popoverOpen: Accessor<boolean>
  onPopoverOpenChange: (open: boolean) => void
  manageOpen: Accessor<boolean>
  setManageOpen: Setter<boolean>
  addOpen: Accessor<boolean>
  setAddOpen: Setter<boolean>
  tab: Accessor<ServerPanelTab>
  onSelectTab: (tab: ServerPanelTab) => void
  search: Accessor<string>
  setSearch: Setter<string>
  address: Accessor<string>
  setAddress: Setter<string>
  name: Accessor<string>
  setName: Setter<string>
  username: Accessor<string>
  setUsername: Setter<string>
  password: Accessor<string>
  setPassword: Setter<string>
  setAddressRef: RefSetter<HTMLInputElement>
  servers: Accessor<SavedServer[]>
  filteredServers: Accessor<SavedServer[]>
  skills: Accessor<RemoteSkill[]>
  activeServerUrl: Accessor<string>
  status: Accessor<RemoteStatus | null>
  onOpenManager: () => void
  onShowAddServer: () => void
  onSaveServer: (event: SubmitEvent) => MaybePromise
  onOpenServer: (server: SavedServer) => void
}

export type GitDiffViewMode = "file" | "all"

export type GitViewerController = {
  open: Accessor<boolean>
  onOpenChange: (open: boolean) => void
  loading: Accessor<boolean>
  error: Accessor<string>
  status: Accessor<RemoteGitStatus | null>
  summary: Accessor<RemoteStatus["git_summary"] | null>
  selectedPath: Accessor<string | null>
  setSelectedPath: Setter<string | null>
  viewMode: Accessor<GitDiffViewMode>
  setViewMode: Setter<GitDiffViewMode>
  onRefresh: () => MaybePromise
}

export type HeaderController = {
  setSidebarOpen: Setter<boolean>
  projectPicker: ProjectPickerController
  isEmptyChat: Accessor<boolean>
  onNewSession: (workspacePath?: string) => MaybePromise
  servers: ServerPanelController
  gitViewer: GitViewerController
}

export type ThreadTabsController = {
  tabs: Accessor<RemoteThreadTabs | null>
  switching: Accessor<boolean>
  onSelectTab: (sessionId: string) => MaybePromise
  onOpenSubagentSession: (sessionId: string) => MaybePromise
}

export type ThreadController = {
  setScrollRef: RefSetter<HTMLDivElement>
  setContentRef: RefSetter<HTMLDivElement>
  isAtTop: Accessor<boolean>
  isAtBottom: Accessor<boolean>
  isEmptyChat: Accessor<boolean>
  streaming: Accessor<boolean>
  visibleMessages: Accessor<RemoteMessage[]>
  threadItems: Accessor<ThreadItem[]>
  projectName: Accessor<string>
  mascotFrame: Accessor<string>
  status: Accessor<RemoteStatus | null>
  token: Accessor<string>
  onPreviewImage: (attachment: AttachmentData) => void
  tabs: ThreadTabsController
  isSubagentView: Accessor<boolean>
}

export type ComposerController = {
  pendingPermission: Accessor<RemotePendingPermission | null>
  permissionBusy: Accessor<boolean>
  onAnswerPermission: (response: RemotePermissionResponse) => MaybePromise
  pendingQuestion: Accessor<RemotePendingQuestion | null>
  questionBusy: Accessor<boolean>
  onAnswerQuestion: (answers: string[][]) => MaybePromise
  onCancelQuestion: () => MaybePromise
  onSubmit: (event: SubmitEvent) => MaybePromise
  onDrop: (event: DragEvent & { currentTarget: HTMLFormElement }) => void
  setImageInputRef: RefSetter<HTMLInputElement>
  openImageInput: () => void
  onAddImageFiles: (files: File[]) => MaybePromise
  attachments: Accessor<ComposerAttachment[]>
  attachmentData: Accessor<AttachmentData[]>
  onRemoveAttachment: (id: string) => void
  onPreviewImage: (attachment: AttachmentData) => void
  prompt: Accessor<string>
  promptAttachmentCount: Accessor<number>
  setPromptRef: RefSetter<HTMLTextAreaElement>
  setPromptOverlayRef: RefSetter<HTMLDivElement>
  onPromptInput: (event: InputEvent & { currentTarget: HTMLTextAreaElement }) => void
  onPromptKeyDown: (event: KeyboardEvent & { currentTarget: HTMLTextAreaElement }) => void
  onRefreshCompletion: () => void
  onPromptScroll: (event: Event & { currentTarget: HTMLTextAreaElement }) => void
  onPromptPaste: (event: ClipboardEvent & { currentTarget: HTMLTextAreaElement }) => void
  suggestions: Accessor<RemoteSuggestion[]>
  suggestionIndex: Accessor<number>
  setSuggestionIndex: Setter<number>
  setSuggestionsRef: RefSetter<HTMLDivElement>
  onChooseSuggestion: (suggestion: RemoteSuggestion) => void
  modelOpen: Accessor<boolean>
  onModelOpenChange: (open: boolean) => void
  modelLabel: Accessor<string>
  setModelSearchRef: RefSetter<HTMLInputElement>
  modelQuery: Accessor<string>
  setModelQuery: Setter<string>
  onModelSearchKeyDown: (event: KeyboardEvent & { currentTarget: HTMLInputElement }) => void
  filteredModels: Accessor<RemoteModel[]>
  modelActiveIndex: Accessor<number>
  setModelActiveIndex: Setter<number>
  onSelectModel: (model: RemoteModel) => MaybePromise
  onControlPopoverCloseAutoFocus: (event: Event) => void
  onControlEscape: () => void
  agentOpen: Accessor<boolean>
  onAgentOpenChange: (open: boolean) => void
  onAgentKeyDown: (event: KeyboardEvent) => void
  agentModes: Accessor<string[]>
  agentActiveIndex: Accessor<number>
  setAgentActiveIndex: Setter<number>
  onSelectAgentMode: (agent: string) => MaybePromise
  reasoningOpen: Accessor<boolean>
  onReasoningOpenChange: (open: boolean) => void
  onReasoningKeyDown: (event: KeyboardEvent) => void
  reasoningOptions: Accessor<string[]>
  reasoningLabel: Accessor<string>
  reasoningActiveIndex: Accessor<number>
  setReasoningActiveIndex: Setter<number>
  onSelectReasoningEffort: (effort: string) => MaybePromise
  status: Accessor<RemoteStatus | null>
  streaming: Accessor<boolean>
  canQueuePrompt: Accessor<boolean>
  promptSending: Accessor<boolean>
  enterSubmitsPrompt: Accessor<boolean>
  queuedMessages: Accessor<string[]>
  queueBusy: Accessor<boolean>
  onSendQueuedNow: () => MaybePromise
}

export type CommandPaletteController = {
  rendered: Accessor<boolean>
  closing: Accessor<boolean>
  query: Accessor<string>
  setQuery: Setter<string>
  setInputRef: RefSetter<HTMLInputElement>
  onClose: () => void
  isEmptyChat: Accessor<boolean>
  onNewSession: (workspacePath?: string) => MaybePromise
  projectResults: Accessor<ProjectGroup[]>
  sessionResults: Accessor<RemoteState["sessions"]>
  onSwitchSession: (id: string) => MaybePromise
}

export type RemoteClientUi = {
  themeStyle: Accessor<JSX.CSSProperties>
  pair: PairPanelController
  sidebar: SidebarController
  header: HeaderController
  thread: ThreadController
  composer: ComposerController
  subagentFooter: SubagentFooterController
  commandPalette: CommandPaletteController
  servers: ServerPanelController
  imagePreview: Accessor<ImagePreviewTarget | null>
  onCloseImagePreview: () => void
}

export type SubagentFooterController = {
  tabs: Accessor<RemoteThreadTabs | null>
  streaming: Accessor<boolean>
  onBackToParent: () => MaybePromise
}
