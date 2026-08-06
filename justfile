default:
    just --list

dev:
    cargo r

remote-client-build:
    cd remote-client && bun install && bun run build

remote-host-dev bind="127.0.0.1:8421":
    cargo r -- serve --bind "{{ bind }}"

# Phone on same LAN: http://<this-machine-ip>:4271 (API proxied to {{ api }} on the host)
remote-client-dev api="http://127.0.0.1:8421":
    cd remote-client && CRABCODE_REMOTE_API_ORIGIN="{{ api }}" bun run dev

dist-build *args:
    just remote-client-build
    dist build {{ args }}

preview:
    ./target/release/crabcode

dpreview *args:
    ./target/debug/crabcode {{ args }}

gen-themes *args:
    bun run scripts/gen-themes.ts {{ args }}

bench-agents *args:
    bun run scripts/bench-agents.ts {{ args }}

devdocs:
    gittydocs dev _docs

log:
    tail -f app.log

sync_readme:
    cp README.md npm/README.md

# Release: bump versions, create release commit, and create a git tag.

# Usage: just tag [patch|minor|major]
tag bump="":
    sh scripts/tag_and_release.sh {{ bump }}
