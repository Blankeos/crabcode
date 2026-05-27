default:
    just --list

dev:
    cargo r

preview:
    ./target/release/crabcode

dpreview:
    ./target/debug/crabcode

gen-themes:
    bun run scripts/gen-themes.ts

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
