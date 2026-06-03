import type { ProjectGroup } from "../../components/remote/project-list"
import type { RemoteState } from "../../remote-api"
import { basename } from "./shared-utils"

export function projectsFromState(state: RemoteState | null | undefined): ProjectGroup[] {
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
