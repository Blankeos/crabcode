# Performance notes

Startup + idle-CPU vs peer agent CLIs. Update this file after meaningful runs.

```bash
just bench-perf
# Prefer the release binary for fair idle numbers:
cargo build --release && PATH="./target/release:$PATH" just bench-perf
```

Script: [`scripts/bench-perf.py`](scripts/bench-perf.py) · recipe: `just bench-perf`

| Section | What | Tool |
| --- | --- | --- |
| **A** | `--version` startup | hyperfine |
| **B** | TUI first frame | PTY + terminal-query replies |
| **C** | Idle CPU after settle | process-tree `ps` (macOS) / `/proc` (Linux) |

Related: [#28](https://github.com/Blankeos/crabcode/issues/28) (idle CPU peg).

---

## Latest

**2026-08-26** · darwin · `Carlos-MacBook-Pro.local` · cwd = repo  
settle=`3s` · sample=`8s` · interval=`0.25s` · version runs=`50`

### A) `--version` (lower is better)

| Agent | mean ± σ | min … max |
| --- | ---: | ---: |
| **crabcode** | **7.71 ms ± 0.88** | 6.82 … 10.81 |
| grok | 10.45 ms ± 0.57 | 9.55 … 12.21 |
| codex | 12.61 ms ± 0.93 | 10.24 … 14.84 |
| opencode | 355.67 ms ± 7.51 | 348.53 … 381.50 |

crabcode is **1.35×** faster than grok, **1.64×** than codex, **~46×** than opencode.

### B) TUI first frame (lower is better)

| Agent | mean | best … worst |
| --- | ---: | ---: |
| **codex** | **50.9 ms** | 50.5 … 51.6 |
| crabcode | 54.9 ms | 54.8 … 55.0 |
| opencode | 1023.7 ms | 969.5 … 1125.0 |
| grok | 1743.3 ms | 1554.0 … 1936.3 |

### C) Idle CPU after settle (lower is better)

| Agent | mean | p50 | p95 | max | RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| **crabcode** | **0.1%** | 0.0% | 0.3% | 1.3% | 52.4 MB |
| grok | 1.0% | 1.0% | 1.4% | 1.5% | 95.9 MB |
| codex | 1.0% | 0.7% | 2.3% | 4.6% | 204.3 MB |
| opencode | 8.8% | 4.8% | 28.5% | 49.3% | 1027.5 MB |

**Verdict:** crabcode best (or tied) on idle CPU.

> Tip: use a **release** binary and `--settle 5 --sample 10+`. Debug builds / short settle can still show Home blink (~60fps) and inflate idle %.

<details>
<summary>Raw dump</summary>

```
A) --version startup (hyperfine)
  crabcode     7.71 ms ±  0.88  (min 6.82, max 10.81, n=50)
  codex       12.61 ms ±  0.93  (min 10.24, max 14.84, n=50)
  grok        10.45 ms ±  0.57  (min 9.55, max 12.21, n=50)
  opencode   355.67 ms ±  7.51  (min 348.53, max 381.50, n=50)

B) TUI first frame
  crabcode   first_frame    54.9 ms  (best 54.8, worst 55.0)
  codex      first_frame    50.9 ms  (best 50.5, worst 51.6)
  grok       first_frame  1743.3 ms  (best 1554.0, worst 1936.3)
  opencode   first_frame  1023.7 ms  (best 969.5, worst 1125.0)

C) Idle CPU (settle=3s, sample=8s)
  crabcode   cpu mean=  0.1%  p50=  0.0%  p95=  0.3%  max=  1.3%  rss=  52.4MB  procs=1  n=25
  codex      cpu mean=  1.0%  p50=  0.7%  p95=  2.3%  max=  4.6%  rss= 204.3MB  procs=1  n=25
  grok       cpu mean=  1.0%  p50=  1.0%  p95=  1.4%  max=  1.5%  rss=  95.9MB  procs=1  n=25
  opencode   cpu mean=  8.8%  p50=  4.8%  p95= 28.5%  max= 49.3%  rss=1027.5MB  procs=1  n=26
```

</details>

---

## History

<details>
<summary>2026-08-26 · darwin · `Carlos-MacBook-Pro.local` · cwd = repo</summary>

**2026-08-26** · darwin · `Carlos-MacBook-Pro.local` · cwd = repo  
settle=`3s` · sample=`8s` · interval=`0.25s` · version runs=`50`

### A) `--version` (lower is better)

| Agent | mean ± σ | min … max |
| --- | ---: | ---: |
| **crabcode** | **8.98 ms ± 1.33** | 6.53 … 14.01 |
| grok | 12.74 ms ± 1.49 | 10.35 … 17.83 |
| codex | 15.24 ms ± 1.18 | 12.58 … 18.59 |
| opencode | 360.30 ms ± 7.27 | 349.46 … 385.32 |

crabcode is **1.42×** faster than grok, **1.70×** than codex, **~40×** than opencode.

### B) TUI first frame (lower is better)

| Agent | mean | best … worst |
| --- | ---: | ---: |
| **codex** | **50.7 ms** | 50.3 … 50.9 |
| crabcode | 54.6 ms | 53.7 … 55.1 |
| opencode | 914.3 ms | 908.1 … 924.6 |
| grok | 1518.7 ms | 1303.7 … 1843.2 |

### C) Idle CPU after settle (lower is better)

| Agent | mean | p50 | p95 | max | RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| **crabcode** | **0.1%** | 0.0% | 0.3% | 1.4% | 52.3 MB |
| grok | 0.8% | 0.9% | 1.2% | 2.1% | 96.8 MB |
| codex | 0.9% | 0.7% | 2.3% | 2.6% | 209.5 MB |
| opencode | 5.2% | 2.5% | 21.7% | 27.0% | 999.1 MB |

**Verdict:** crabcode best (or tied) on idle CPU.

> Tip: use a **release** binary and `--settle 5 --sample 10+`. Debug builds / short settle can still show Home blink (~60fps) and inflate idle %.

<details>
<summary>Raw dump</summary>

```
A) --version startup (hyperfine)
  crabcode     8.98 ms ±  1.33  (min 6.53, max 14.01, n=50)
  codex       15.24 ms ±  1.18  (min 12.58, max 18.59, n=50)
  grok        12.74 ms ±  1.49  (min 10.35, max 17.83, n=50)
  opencode   360.30 ms ±  7.27  (min 349.46, max 385.32, n=50)

B) TUI first frame
  crabcode   first_frame    54.6 ms  (best 53.7, worst 55.1)
  codex      first_frame    50.7 ms  (best 50.3, worst 50.9)
  grok       first_frame  1518.7 ms  (best 1303.7, worst 1843.2)
  opencode   first_frame   914.3 ms  (best 908.1, worst 924.6)

C) Idle CPU (settle=3s, sample=8s)
  crabcode   cpu mean=  0.1%  p50=  0.0%  p95=  0.3%  max=  1.4%  rss=  52.3MB  procs=1  n=26
  codex      cpu mean=  0.9%  p50=  0.7%  p95=  2.3%  max=  2.6%  rss= 209.5MB  procs=1  n=26
  grok       cpu mean=  0.8%  p50=  0.9%  p95=  1.2%  max=  2.1%  rss=  96.8MB  procs=1  n=26
  opencode   cpu mean=  5.2%  p50=  2.5%  p95= 21.7%  max= 27.0%  rss= 999.1MB  procs=1  n=26
```

</details>

</details>

<details>
<summary>2026-08-26 · darwin · `Carlos-MacBook-Pro.local` · cwd = repo</summary>

**2026-08-26** · darwin · `Carlos-MacBook-Pro.local` · cwd = repo  
settle=`3s` · sample=`8s` · interval=`0.25s` · version runs=`50`

### A) `--version` (lower is better)

| Agent | mean ± σ | min … max |
| --- | ---: | ---: |
| **crabcode** | **8.47 ms ± 1.91** | 6.46 … 18.29 |
| codex | 12.71 ms ± 1.31 | 11.11 … 18.57 |
| grok | 12.86 ms ± 1.49 | 9.66 … 15.58 |
| opencode | 367.74 ms ± 13.62 | 352.03 … 430.16 |

crabcode is **1.50×** faster than codex, **1.52×** than grok, **~43×** than opencode.

### B) TUI first frame (lower is better)

| Agent | mean | best … worst |
| --- | ---: | ---: |
| **codex** | **70.7 ms** | 50.4 … 110.0 |
| crabcode | 123.1 ms | 106.2 … 156.8 |
| opencode | 1097.4 ms | 973.4 … 1342.9 |
| grok | 1537.8 ms | 1347.4 … 1650.7 |

### C) Idle CPU after settle (lower is better)

| Agent | mean | p50 | p95 | max | RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| **crabcode** | **0.1%** | 0.0% | 0.2% | 1.0% | 51.9 MB |
| grok | 0.9% | 0.9% | 1.6% | 1.7% | 97.9 MB |
| codex | 1.0% | 0.6% | 2.6% | 5.1% | 192.4 MB |
| opencode | 5.6% | 1.5% | 27.3% | 33.9% | 990.1 MB |

**Verdict:** crabcode best (or tied) on idle CPU.

> Tip: use a **release** binary and `--settle 5 --sample 10+`. Debug builds / short settle can still show Home blink (~60fps) and inflate idle %.

<details>
<summary>Raw dump</summary>

```
A) --version startup (hyperfine)
  crabcode     8.47 ms ±  1.91  (min 6.46, max 18.29, n=50)
  codex       12.71 ms ±  1.31  (min 11.11, max 18.57, n=50)
  grok        12.86 ms ±  1.49  (min 9.66, max 15.58, n=50)
  opencode   367.74 ms ± 13.62  (min 352.03, max 430.16, n=50)

B) TUI first frame
  crabcode   first_frame   123.1 ms  (best 106.2, worst 156.8)
  codex      first_frame    70.7 ms  (best 50.4, worst 110.0)
  grok       first_frame  1537.8 ms  (best 1347.4, worst 1650.7)
  opencode   first_frame  1097.4 ms  (best 973.4, worst 1342.9)

C) Idle CPU (settle=3s, sample=8s)
  crabcode   cpu mean=  0.1%  p50=  0.0%  p95=  0.2%  max=  1.0%  rss=  51.9MB  procs=1  n=26
  codex      cpu mean=  1.0%  p50=  0.6%  p95=  2.6%  max=  5.1%  rss= 192.4MB  procs=1  n=26
  grok       cpu mean=  0.9%  p50=  0.9%  p95=  1.6%  max=  1.7%  rss=  97.9MB  procs=1  n=26
  opencode   cpu mean=  5.6%  p50=  1.5%  p95= 27.3%  max= 33.9%  rss= 990.1MB  procs=1  n=26
```

</details>

</details>

<details>
<summary>2026-08-26 · darwin · `Carlos-MacBook-Pro.local` · cwd = repo</summary>

**2026-08-26** · darwin · `Carlos-MacBook-Pro.local` · cwd = repo  
settle=`3s` · sample=`8s` · interval=`0.25s` · version runs=`50`

### A) `--version` (lower is better)

| Agent | mean ± σ | min … max |
| --- | ---: | ---: |
| **crabcode** | **7.09 ms ± 0.83** | 5.84 … 9.76 |
| codex | 12.53 ms ± 0.96 | 11.56 … 15.46 |
| grok | 12.60 ms ± 2.40 | 10.23 … 18.87 |
| opencode | 361.40 ms ± 6.30 | 352.33 … 378.60 |

crabcode is **1.77×** faster than codex, **1.78×** than grok, **~51×** than opencode.

### B) TUI first frame (lower is better)

| Agent | mean | best … worst |
| --- | ---: | ---: |
| **codex** | **50.8 ms** | 50.1 … 51.4 |
| crabcode | 103.4 ms | 101.8 … 105.2 |
| opencode | 1000.0 ms | 907.2 … 1126.8 |
| grok | 1684.7 ms | 1394.4 … 2103.6 |

### C) Idle CPU after settle (lower is better)

| Agent | mean | p50 | p95 | max | RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| **crabcode** | **0.1%** | 0.0% | 0.3% | 1.7% | 51.8 MB |
| codex | 0.6% | 0.4% | 2.3% | 4.3% | 195.5 MB |
| grok | 1.0% | 1.0% | 1.5% | 1.8% | 103.6 MB |
| opencode | 7.1% | 2.6% | 18.3% | 84.4% | 1020.1 MB |

**Verdict:** crabcode best (or tied) on idle CPU.

> Tip: use a **release** binary and `--settle 5 --sample 10+`. Debug builds / short settle can still show Home blink (~60fps) and inflate idle %.

<details>
<summary>Raw dump</summary>

```
A) --version startup (hyperfine)
  crabcode     7.09 ms ±  0.83  (min 5.84, max 9.76, n=50)
  codex       12.53 ms ±  0.96  (min 11.56, max 15.46, n=50)
  grok        12.60 ms ±  2.40  (min 10.23, max 18.87, n=50)
  opencode   361.40 ms ±  6.30  (min 352.33, max 378.60, n=50)

B) TUI first frame
  crabcode   first_frame   103.4 ms  (best 101.8, worst 105.2)
  codex      first_frame    50.8 ms  (best 50.1, worst 51.4)
  grok       first_frame  1684.7 ms  (best 1394.4, worst 2103.6)
  opencode   first_frame  1000.0 ms  (best 907.2, worst 1126.8)

C) Idle CPU (settle=3s, sample=8s)
  crabcode   cpu mean=  0.1%  p50=  0.0%  p95=  0.3%  max=  1.7%  rss=  51.8MB  procs=1  n=25
  codex      cpu mean=  0.6%  p50=  0.4%  p95=  2.3%  max=  4.3%  rss= 195.5MB  procs=1  n=25
  grok       cpu mean=  1.0%  p50=  1.0%  p95=  1.5%  max=  1.8%  rss= 103.6MB  procs=1  n=25
  opencode   cpu mean=  7.1%  p50=  2.6%  p95= 18.3%  max= 84.4%  rss=1020.1MB  procs=1  n=25
```

</details>

</details>

<details>
<summary>2026-08-26 · darwin · `Carlos-MacBook-Pro.local` · cwd = repo</summary>

**2026-08-26** · darwin · `Carlos-MacBook-Pro.local` · cwd = repo  
settle=`3s` · sample=`8s` · interval=`0.25s` · version runs=`50`

### A) `--version` (lower is better)

| Agent | mean ± σ | min … max |
| --- | ---: | ---: |
| **crabcode** | **7.66 ms ± 0.67** | 6.42 … 9.14 |
| grok | 12.97 ms ± 2.00 | 10.31 … 20.68 |
| codex | 14.28 ms ± 1.49 | 11.78 … 19.44 |
| opencode | 366.29 ms ± 7.38 | 356.21 … 388.76 |

crabcode is **1.69×** faster than grok, **1.86×** than codex, **~48×** than opencode.

### B) TUI first frame (lower is better)

| Agent | mean | best … worst |
| --- | ---: | ---: |
| **codex** | **53.0 ms** | 51.3 … 55.1 |
| crabcode | 121.5 ms | 101.9 … 159.9 |
| opencode | 1020.7 ms | 967.7 … 1126.1 |
| grok | 1771.7 ms | 1505.9 … 2145.1 |

### C) Idle CPU after settle (lower is better) · [#28](https://github.com/Blankeos/crabcode/issues/28)

| Agent | mean | p50 | p95 | max | RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| **codex** | **0.0%** | 0.0% | 0.0% | 0.1% | 48.8 MB |
| grok | 0.9% | 0.9% | 1.4% | 1.5% | 97.4 MB |
| crabcode | 3.3% | 3.3% | 3.9% | 4.0% | 51.9 MB |
| opencode | 5.2% | 2.4% | 13.3% | 42.9% | 1014.8 MB |

**Verdict:** loses idle-CPU to codex + grok on this run. Aim: mean ≤ best peer (and ≪ 100% on Linux for #28).

> Tip: use a **release** binary and `--settle 5 --sample 10+`. Debug builds / short settle can still show Home blink (~60fps) and inflate idle %.

<details>
<summary>Raw dump</summary>

```
A) --version startup (hyperfine)
  crabcode      7.66 ms ±  0.67  (min 6.42, max 9.14, n=50)
  codex        14.28 ms ±  1.49  (min 11.78, max 19.44, n=50)
  grok         12.97 ms ±  2.00  (min 10.31, max 20.68, n=50)
  opencode    366.29 ms ±  7.38  (min 356.21, max 388.76, n=50)

B) TUI first frame
  crabcode   first_frame   121.5 ms  (best 101.9, worst 159.9, n=3)
  codex      first_frame    53.0 ms  (best 51.3, worst 55.1, n=3)
  grok       first_frame  1771.7 ms  (best 1505.9, worst 2145.1, n=3)
  opencode   first_frame  1020.7 ms  (best 967.7, worst 1126.1, n=3)

C) Idle CPU (settle=3.0s, sample=8.0s)
  crabcode   cpu mean=  3.3%  p50=  3.3%  p95=  3.9%  max=  4.0%  rss=  51.9MB  procs=1  n=25
  codex      cpu mean=  0.0%  p50=  0.0%  p95=  0.0%  max=  0.1%  rss=  48.8MB  procs=1  n=25
  grok       cpu mean=  0.9%  p50=  0.9%  p95=  1.4%  max=  1.5%  rss=  97.4MB  procs=1  n=25
  opencode   cpu mean=  5.2%  p50=  2.4%  p95= 13.3%  max= 42.9%  rss=1014.8MB  procs=1  n=25
```

</details>

</details>

---

## How to refresh

1. `cargo build --release`
2. `PATH="./target/release:$PATH" just bench-perf --settle 5 --sample 10 --runs 50`
3. Answer **`y`** to `Add this to PERF.md?` (or pass `--write-perf` / `--no-write-perf`).
4. Optional JSON: `--json-out /tmp/crabcode-bench-perf.json`
