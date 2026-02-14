default:
    just --list

dev:
    cargo r

preview:
    ./target/release/crabcode

gen-themes:
    bun run scripts/gen-themes.ts

devdocs:
    gittydocs dev _docs

log:
    tail -f app.log

# Release: bump versions, create release commit, and create a git tag.
# Usage: just tag [patch|minor|major]
tag bump="":
    sh scripts/tag_and_release.sh {{bump}}
