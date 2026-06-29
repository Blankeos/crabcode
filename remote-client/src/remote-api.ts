export type RemoteStatus = {
  version: string
  workspace: string
  cwd: string
  provider: string
  model: string
  agent: string
  primary_agents: string[]
  reasoning_effort: string | null
  reasoning_efforts: string[]
  browser_url: string
  suggested_alias: string
  auth_required: boolean
  pair_expires_at: number
  theme: RemoteTheme
  git_summary: RemoteGitSummary
}

export type RemoteTheme = {
  primary: string
  primary_dim: string
}

export type RemoteGitSummary = {
  is_repo: boolean
  branch: string | null
}

export type RemoteGitStatus = {
  is_repo: boolean
  branch: string | null
  changed_files: number
  additions: number
  deletions: number
  files: RemoteGitFileChange[]
  diff_files: RemoteGitDiffFile[]
  truncated: boolean
}

export type RemoteGitFileChange = {
  path: string
  old_path: string | null
  status: string
  additions: number
  deletions: number
  binary: boolean
}

export type RemoteGitDiffFile = {
  path: string
  old_path: string | null
  status: string
  additions: number
  deletions: number
  binary: boolean
  lines: RemoteGitDiffLine[]
  truncated: boolean
}

export type RemoteGitDiffLine = {
  kind: "add" | "remove" | "context" | "hunk" | "meta" | string
  text: string
  old_line: number | null
  new_line: number | null
}

export type RemoteSession = {
  id: string
  parent_id: string | null
  title: string
  workspace: string
  workspace_path: string
  status: string
  message_count: number
  updated_at: number
}

export type RemoteWorkspace = {
  name: string
  path: string
  sort_order: number
  last_opened_at: number
}

export type RemoteJsonValue =
  | null
  | boolean
  | number
  | string
  | RemoteJsonValue[]
  | { [key: string]: RemoteJsonValue }

export type RemoteMessagePart = {
  type: string
  [key: string]: RemoteJsonValue
}

export type RemoteMessage = {
  role: "user" | "assistant" | "system" | "tool" | string
  content: string
  reasoning: string | null
  parts?: RemoteMessagePart[]
  is_complete: boolean
  agent_mode: string | null
  token_count: number | null
  duration_ms: number | null
  t0_ms: number | null
  t1_ms: number | null
  tn_ms: number | null
  output_tokens: number | null
  model: string | null
  provider: string | null
  local_image_paths: string[]
  was_interrupted: boolean
}

export type RemotePendingPermission = {
  tool_id: string
  action: string
  target: string | null
  command: string | null
  workdir: string | null
  reason: string
  queued_count: number
}

export type RemotePendingQuestion = {
  questions: RemoteQuestionItem[]
  queued_count: number
}

export type RemoteQuestionItem = {
  header: string
  question: string
  options: RemoteQuestionOption[]
  multiple: boolean
  custom: boolean
}

export type RemoteQuestionOption = {
  label: string
  description: string
}

export type RemoteThreadTab = {
  session_id: string
  label: string
  agent: string
  model: string
  active: boolean
  running: boolean
  kind: "main" | "subagent" | string
  accent: string
}

export type RemoteThreadTabs = {
  root_session_id: string
  is_child_session: boolean
  tabs: RemoteThreadTab[]
}

export type RemoteState = {
  status: RemoteStatus
  projects: RemoteWorkspace[]
  sessions: RemoteSession[]
  current_session_id: string | null
  messages: RemoteMessage[]
  is_streaming: boolean
  queued_messages: string[]
  pending_permission: RemotePendingPermission | null
  pending_question: RemotePendingQuestion | null
  thread_tabs: RemoteThreadTabs | null
}

export type RemoteModel = {
  id: string
  name: string
  group: string
  description: string
  provider_id: string
  active: boolean
  favorite: boolean
}

export type RemoteSuggestion = {
  name: string
  description: string
  replacement: string
  kind: "command" | "agent" | "file"
  is_directory: boolean
}

export type RemoteSkill = {
  name: string
  description: string
  location: string
}

export type RemoteMcpServer = {
  name: string
  enabled: boolean
  status: string
  kind: string
}

export type RemotePromptImage = {
  name: string
  media_type: string
  data_url: string
}

export type PairResponse = {
  token: string
  suggested_alias: string
  workspace_label: string
  browser_url: string
}

export class RemoteApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly body: string
  ) {
    super(message)
    this.name = "RemoteApiError"
  }
}

function responseErrorMessage(body: string) {
  try {
    const parsed = JSON.parse(body) as { error?: unknown; message?: unknown }
    if (typeof parsed.error === "string") return parsed.error
    if (typeof parsed.message === "string") return parsed.message
  } catch {
    // Non-JSON responses fall through to the raw body.
  }

  return body || "Request failed."
}

export function createRemoteApi(getToken: () => string) {
  const headers = (body?: unknown) => {
    const token = getToken()
    return {
      ...(body ? { "Content-Type": "application/json" } : {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    }
  }

  async function request<T>(path: string, options: RequestInit & { json?: unknown } = {}) {
    const response = await fetch(path, {
      ...options,
      body: options.json === undefined ? options.body : JSON.stringify(options.json),
      headers: {
        ...headers(options.json ?? options.body),
        ...(options.headers || {}),
      },
    })

    if (!response.ok) {
      const body = await response.text()
      throw new RemoteApiError(responseErrorMessage(body), response.status, body)
    }

    return response.json() as Promise<T>
  }

  return {
    status: () => request<RemoteStatus>("/api/status"),
    state: () => request<RemoteState>("/api/state"),
    gitStatus: () => request<RemoteGitStatus>("/api/git/status"),
    stateEvents: (
      onState: (state: RemoteState) => void,
      onError?: (error: Event) => void
    ) => {
      const url = new URL("/api/events", window.location.origin)
      const token = getToken()
      if (token) url.searchParams.set("token", token)

      const source = new EventSource(url)
      source.addEventListener("state", (event) => {
        onState(JSON.parse((event as MessageEvent<string>).data) as RemoteState)
      })
      source.onerror = (event) => onError?.(event)
      return () => source.close()
    },
    pair: (code: string) =>
      request<PairResponse>("/api/pair", {
        method: "POST",
        json: { code, role: "phone-browser" },
      }),
    newSession: (workspace_path?: string) =>
      request<RemoteState>("/api/session/new", {
        method: "POST",
        json: { workspace_path },
      }),
    selectWorkspace: (path: string) =>
      request<RemoteState>("/api/workspace/select", { method: "POST", json: { path } }),
    switchSession: (id: string) =>
      request<RemoteState>("/api/session/switch", { method: "POST", json: { id } }),
    archiveSession: (id: string) =>
      request<RemoteState>("/api/session/archive", { method: "POST", json: { id } }),
    archiveWorkspace: (path: string) =>
      request<RemoteState>("/api/workspace/archive", { method: "POST", json: { path } }),
    prompt: (prompt: string, images: RemotePromptImage[] = []) =>
      request<{ session_id: string }>("/api/prompt", {
        method: "POST",
        json: { prompt, images },
      }),
    autocomplete: (trigger: "slash" | "mention", query: string, is_chat: boolean) =>
      request<RemoteSuggestion[]>("/api/autocomplete", {
        method: "POST",
        json: { trigger, query, is_chat },
      }),
    cancel: () =>
      request<{ cancelled: boolean }>("/api/cancel", { method: "POST", json: {} }),
    sendQueuedNow: () =>
      request<RemoteState>("/api/queue/send-now", { method: "POST", json: {} }),
    answerPermission: (response: "deny" | "allow_once" | "allow_always") =>
      request<RemoteState>("/api/permission", { method: "POST", json: { response } }),
    answerQuestion: (answers: string[][]) =>
      request<RemoteState>("/api/question", { method: "POST", json: { answers } }),
    cancelQuestion: () =>
      request<RemoteState>("/api/question/cancel", { method: "POST", json: {} }),
    models: () => request<RemoteModel[]>("/api/models"),
    skills: () => request<RemoteSkill[]>("/api/skills"),
    mcp: () => request<RemoteMcpServer[]>("/api/mcp"),
    mcpToggle: (name: string) =>
      request<RemoteMcpServer[]>("/api/mcp/toggle", { method: "POST", json: { name } }),
    selectModel: (provider_id: string, model_id: string) =>
      request<RemoteStatus>("/api/model", {
        method: "POST",
        json: { provider_id, model_id },
      }),
    toggleAgent: () => request<RemoteState>("/api/agent/toggle", { method: "POST", json: {} }),
    setAgent: (agent: string) =>
      request<RemoteState>("/api/agent", { method: "POST", json: { agent } }),
    setReasoning: (effort: string | null) =>
      request<RemoteState>("/api/reasoning", { method: "POST", json: { effort } }),
  }
}
