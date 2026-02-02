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
