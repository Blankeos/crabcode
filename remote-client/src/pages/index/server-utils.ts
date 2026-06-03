import type { SavedServer } from "./page-types"

const SERVERS_KEY = "crabcode.remote.servers"

export function loadSavedServers(): SavedServer[] {
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

export function saveSavedServers(servers: SavedServer[]) {
  localStorage.setItem(SERVERS_KEY, JSON.stringify(servers))
}

export function normalizeServerAddress(address: string) {
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

export function browserOrigin() {
  return typeof window === "undefined" ? "" : window.location.origin
}

export function isActiveServer(address: string, activeAddress: string) {
  return normalizeServerAddress(address) === normalizeServerAddress(activeAddress)
}
