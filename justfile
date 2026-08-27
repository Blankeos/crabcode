default:
    just --list

check-aisdk-boundary:
    bash scripts/check-aisdk-boundary.sh

dev:
    cargo r

remote-client-build:
    cd remote-client && bun install && bun run build

remote-host-dev bind="127.0.0.1:8421":
    cargo r -- serve --bind "{{ bind }}"

[doc('Phone on same LAN: http://<this-machine-ip>:4271 (API proxied to {{ api }} on the host)')]
remote-client-dev api="http://127.0.0.1:8421":
    cd remote-client && CRABCODE_REMOTE_API_ORIGIN="{{ api }}" bun run dev

dist-build *args:
    just remote-client-build
    dist build {{ args }}

preview *args:
    ./target/release/crabcode {{ args }}

dpreview *args:
    ./target/debug/crabcode {{ args }}

gen-themes *args:
    bun run scripts/gen-themes.ts {{ args }}

[doc("""
  Agent self-eval benchmarks (crabcode / opencode / codex / grok-build).

  Pass-through args to scripts/bench-agents.ts. Reports → benchmark-reports/

  just bench-agents
  just bench-agents --model openai/gpt-5.5
  just bench-agents --tasks bugfix-js,add-rust-test --model openai/gpt-5.5
  just bench-agents --agents crabcode,grok-build
  just bench-agents --agents crabcode,grok-build --tasks bugfix-js --model grok-4.5
  BENCH_CRABCODE_REASONING=high just bench-agents --model openai/gpt-5.5
  just bench-agents --list-tasks
  just bench-agents --estimate
  just bench-agents --help

  Crabcode reasoning: BENCH_CRABCODE_REASONING (default medium).
  OpenAI model ids may fail on grok-build — use an xAI model or drop it from --agents.
""")]
bench-agents *args:
    bun run scripts/bench-agents.ts {{ args }}

[doc("""
  Startup + idle-CPU perf vs peer CLIs.

  A) hyperfine --version   B) PTY first-frame   C) idle CPU after settle
  Ends with: Add this to PERF.md? [y/N]

  just bench-perf
  just bench-perf --agents crabcode,codex,grok
  just bench-perf --section idle --settle 5 --sample 15
  just bench-perf --write-perf          # skip prompt, update PERF.md
  just bench-perf --no-write-perf       # skip prompt, don't update
  cargo build --release && PATH="./target/release:$PATH" just bench-perf
""")]
bench-perf *args:
    python3 scripts/bench-perf.py {{ args }}

devdocs:
    gittydocs dev _docs

log:
    tail -f app.log

sync_readme:
    cp README.md npm/README.md

[doc('Release: bump versions, commit, and tag (just tag [patch|minor|major])')]
tag bump="":
    sh scripts/tag_and_release.sh {{ bump }}
