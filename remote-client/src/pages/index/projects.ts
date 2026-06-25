import type { ProjectGroup } from "../../components/remote/project-list"
import type { RemoteSession, RemoteState } from "../../remote-api"
import { basename } from "./shared-utils"

export type ProjectListEntry = ProjectGroup & {
  sort_order: number
  last_opened_at: number
}

export function projectsFromState(state: RemoteState | null | undefined): ProjectGroup[] {
  return projectsWithMetaFromState(state).map(stripProjectMeta)
}

/** Open-project picker: most recently opened first (sidebar keeps `projectsFromState` order). */
export function projectsForPicker(state: RemoteState | null | undefined): ProjectGroup[] {
  const list = projectsWithMetaFromState(state)
  list.sort((a, b) => {
    if (b.last_opened_at !== a.last_opened_at) {
      return b.last_opened_at - a.last_opened_at
    }
    if (a.sort_order !== b.sort_order) {
      return a.sort_order - b.sort_order
    }
    return a.name.localeCompare(b.name) || a.path.localeCompare(b.path)
  })
  return list.map(stripProjectMeta)
}

export function mostRecentSession(sessions: readonly RemoteSession[]): RemoteSession | undefined {
  return sessions.reduce<RemoteSession | undefined>((latest, session) => {
    if (!latest || session.updated_at > latest.updated_at) return session
    return latest
  }, undefined)
}

/** Sidebar accordion key — must match `project.path || project.name` in ProjectList. */
export function sidebarProjectKey(project: Pick<ProjectGroup, "path" | "name">): string {
  return project.path || project.name
}

export function sidebarProjectKeyForActivePath(
  activePath: string,
  projects: readonly ProjectGroup[]
): string | undefined {
  const active = activePath.trim()
  if (!active) return undefined
  const exact = projects.find((p) => p.path.trim() === active)
  if (exact) return sidebarProjectKey(exact)
  const suffix = projects.find((p) => {
    const path = p.path.trim()
    return path && (active === path || active.endsWith(`/${path}`) || active.endsWith(path))
  })
  if (suffix) return sidebarProjectKey(suffix)
  return active
}

function projectsWithMetaFromState(state: RemoteState | null | undefined): ProjectListEntry[] {
  const map = new Map<string, ProjectListEntry>()
  const currentPath = state?.status.cwd || ""

  for (const project of state?.projects ?? []) {
    const path = project.path || project.name
    const key = path || project.name || "Workspace"
    if (!key || map.has(key)) continue

    map.set(key, {
      name: project.name || basename(path) || "Workspace",
      path,
      sessions: [],
      sort_order: project.sort_order ?? 0,
      last_opened_at: project.last_opened_at ?? 0,
    })
  }

  for (const session of state?.sessions ?? []) {
    const path = session.workspace_path || session.workspace || state?.status.cwd || "Workspace"
    const key = path || session.workspace || "Workspace"
    const current = map.get(key) ?? {
      name: session.workspace || basename(path) || state?.status.workspace || "Workspace",
      path,
      sessions: [],
      sort_order: 0,
      last_opened_at: 0,
    }
    map.set(key, { ...current, sessions: [...current.sessions, session] })
  }

  if (currentPath && !map.has(currentPath)) {
    map.set(currentPath, {
      name: state?.status.workspace || basename(currentPath) || "Workspace",
      path: currentPath,
      sessions: [],
      sort_order: 0,
      last_opened_at: Number.MAX_SAFE_INTEGER,
    })
  }

  if (map.size === 0 && state?.status.workspace) {
    map.set(state.status.workspace, {
      name: state.status.workspace,
      path: state.status.cwd || state.status.workspace,
      sessions: [],
      sort_order: 0,
      last_opened_at: 0,
    })
  }
  return [...map.values()]
}

function stripProjectMeta(entry: ProjectListEntry): ProjectGroup {
  return {
    name: entry.name,
    path: entry.path,
    sessions: entry.sessions,
  }
}