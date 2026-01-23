default:
    just --list

dev:
    cargo r

preview:
    ./target/release/crabcode

gen-themes:
    bun run scripts/gen-themes.ts
