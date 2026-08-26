#!/usr/bin/env python3
"""Compare crabcode vs peer agent CLIs on startup + idle CPU.

Sections
  A) --version startup (hyperfine, lazygitrs-style)
  B) TUI open / first-frame (PTY + terminal-query replies)
  C) Idle CPU after settle (process tree via ps / optional /proc)

Examples
  just bench-perf
  python3 scripts/bench-perf.py --agents crabcode,codex,grok,opencode
  python3 scripts/bench-perf.py --section version
  python3 scripts/bench-perf.py --section idle --settle 5 --sample 10
  python3 scripts/bench-perf.py --cwd /tmp --json-out /tmp/bench.json

Requires: python3. Optional: hyperfine (section A).
"""

from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import termios
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

DEFAULT_AGENTS = ("crabcode", "codex", "grok", "opencode")

# argv to launch interactive TUI (no prompt / no print mode)
AGENT_ARGV: dict[str, list[str]] = {
    "crabcode": ["crabcode"],
    "codex": ["codex"],
    "grok": ["grok"],
    "opencode": ["opencode"],
}

AGENT_VERSION_ARGV: dict[str, list[str]] = {
    "crabcode": ["crabcode", "--version"],
    "codex": ["codex", "--version"],
    "grok": ["grok", "--version"],
    "opencode": ["opencode", "--version"],
}


@dataclass
class VersionResult:
    agent: str
    available: bool
    mean_ms: float | None = None
    stddev_ms: float | None = None
    min_ms: float | None = None
    max_ms: float | None = None
    runs: int | None = None
    error: str | None = None
    raw: str | None = None


@dataclass
class OpenResult:
    agent: str
    available: bool
    first_byte_ms: float | None = None
    first_frame_ms: float | None = None
    best_ms: float | None = None
    worst_ms: float | None = None
    bytes_seen: int = 0
    error: str | None = None


@dataclass
class IdleSample:
    cpu_pct: float
    rss_kb: int
    nprocs: int


@dataclass
class IdleResult:
    agent: str
    available: bool
    first_frame_ms: float | None = None
    cpu_mean: float | None = None
    cpu_p50: float | None = None
    cpu_p95: float | None = None
    cpu_max: float | None = None
    cpu_min: float | None = None
    rss_mean_mb: float | None = None
    nprocs: int | None = None
    samples: int = 0
    error: str | None = None


@dataclass
class Report:
    host: str
    platform: str
    cwd: str
    settle_s: float
    sample_s: float
    sample_interval_s: float
    version: list[VersionResult] = field(default_factory=list)
    open: list[OpenResult] = field(default_factory=list)
    idle: list[IdleResult] = field(default_factory=list)


def which_agent(name: str) -> str | None:
    argv = AGENT_ARGV.get(name, [name])
    return shutil.which(argv[0])


def set_winsize(fd: int, rows: int = 40, cols: int = 120) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def reply_queries(data: bytes) -> bytes:
    """Answer common DA / cursor / OSC probes so TUIs leave the probe phase."""
    out = b""
    if b"\x1b[c" in data or b"\x1b[0c" in data:
        out += b"\x1b[?62;22c"
    if b"\x1b[>c" in data or b"\x1b[>0c" in data:
        out += b"\x1b[>1;10;0c"
    if b"\x1b[6n" in data:
        out += b"\x1b[24;80R"
    if b"\x1b[>0q" in data:
        out += b"\x1bP>|xterm-256color\x1b\\"
    if b"\x1b]10;?" in data:
        out += b"\x1b]10;rgb:aaaa/aaaa/aaaa\x1b\\"
    if b"\x1b]11;?" in data:
        out += b"\x1b]11;rgb:1111/1111/1111\x1b\\"
    return out


def looks_like_frame(buf: bytes) -> bool:
    if len(buf) < 400:
        return False
    markers = (b"\x1b[2J", b"\x1b[H", b"\x1b[?1049h", b"\x1b[?2026h", b"\x1b[?1049h")
    return any(m in buf for m in markers)


def descendant_pids(root: int) -> set[int]:
    pids = {root}
    queue = [root]
    while queue:
        parent = queue.pop()
        try:
            out = subprocess.check_output(
                ["pgrep", "-P", str(parent)], text=True, stderr=subprocess.DEVNULL
            )
        except (subprocess.CalledProcessError, FileNotFoundError):
            continue
        for line in out.split():
            try:
                child = int(line)
            except ValueError:
                continue
            if child not in pids:
                pids.add(child)
                queue.append(child)
    return pids


def read_cpu_rss(pids: set[int]) -> tuple[float, int, int]:
    """Return (cpu_pct_sum, rss_kb_sum, alive_count).

    On Linux, prefer /proc/<pid>/stat delta when available for more stable
    instantaneous CPU; fall back to `ps -o %cpu` everywhere else.
    """
    if sys.platform.startswith("linux"):
        try:
            return _linux_cpu_rss(pids)
        except Exception:
            pass
    return _ps_cpu_rss(pids)


def _ps_cpu_rss(pids: set[int]) -> tuple[float, int, int]:
    total_cpu = 0.0
    total_rss = 0
    alive = 0
    for pid in list(pids):
        try:
            out = subprocess.check_output(
                ["ps", "-p", str(pid), "-o", "%cpu=,rss="],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
        except subprocess.CalledProcessError:
            continue
        if not out:
            continue
        parts = out.split()
        if len(parts) < 2:
            continue
        try:
            total_cpu += float(parts[0])
            total_rss += int(parts[1])
            alive += 1
        except ValueError:
            continue
    return total_cpu, total_rss, alive


_linux_prev: dict[int, tuple[int, int, float]] = {}


def _linux_cpu_rss(pids: set[int]) -> tuple[float, int, int]:
    """Approximate %CPU over the last sample interval using /proc jiffies."""
    global _linux_prev
    clk = os.sysconf(os.sysconf_names.get("SC_CLK_TCK", "SC_CLK_TCK"))
    now = time.time()
    total_cpu = 0.0
    total_rss = 0
    alive = 0
    seen: set[int] = set()
    for pid in list(pids):
        stat_path = Path(f"/proc/{pid}/stat")
        status_path = Path(f"/proc/{pid}/status")
        if not stat_path.exists():
            continue
        try:
            fields = stat_path.read_text().rsplit(")", 1)[-1].split()
            # After comm: utime=14th field of full stat → index 11 in post-comm
            utime = int(fields[11])
            stime = int(fields[12])
            jiffies = utime + stime
        except Exception:
            continue
        rss_kb = 0
        try:
            for line in status_path.read_text().splitlines():
                if line.startswith("VmRSS:"):
                    rss_kb = int(line.split()[1])
                    break
        except Exception:
            pass
        prev = _linux_prev.get(pid)
        if prev is not None:
            prev_j, _prev_rss, prev_t = prev
            dt = max(now - prev_t, 1e-6)
            dj = max(jiffies - prev_j, 0)
            total_cpu += (dj / clk) / dt * 100.0
        _linux_prev[pid] = (jiffies, rss_kb, now)
        seen.add(pid)
        total_rss += rss_kb
        alive += 1
    # Drop stale
    for pid in list(_linux_prev):
        if pid not in seen:
            _linux_prev.pop(pid, None)
    return total_cpu, total_rss, alive


def kill_tree(proc: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=2)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=1)
    except Exception:
        pass


def spawn_pty(argv: list[str], cwd: str) -> tuple[subprocess.Popen[bytes], int]:
    master, slave = pty.openpty()
    set_winsize(slave)
    set_winsize(master)
    env = os.environ.copy()
    env.setdefault("TERM", "xterm-256color")
    env.setdefault("COLORTERM", "truecolor")
    # Keep auth out of the way for idle benches when possible
    env.setdefault("CI", "1")
    proc = subprocess.Popen(
        argv,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        cwd=cwd,
        preexec_fn=os.setsid,
    )
    os.close(slave)
    return proc, master


def drain_and_reply(master: int, buf: bytearray, timeout: float = 0.0) -> int:
    """Read available PTY output, reply to queries. Returns bytes read."""
    read_n = 0
    end = time.perf_counter() + timeout
    while True:
        wait = max(0.0, end - time.perf_counter()) if timeout else 0.0
        r, _, _ = select.select([master], [], [], wait)
        if master not in r:
            break
        try:
            chunk = os.read(master, 65536)
        except OSError:
            break
        if not chunk:
            break
        buf.extend(chunk)
        read_n += len(chunk)
        reply = reply_queries(chunk)
        if reply:
            try:
                os.write(master, reply)
            except OSError:
                pass
        if timeout == 0.0:
            # non-blocking drain: keep going while data ready
            continue
        if time.perf_counter() >= end:
            break
    return read_n


def wait_first_frame(
    proc: subprocess.Popen[bytes], master: int, timeout: float
) -> tuple[float | None, float | None, bytearray]:
    buf = bytearray()
    start = time.perf_counter()
    first_byte: float | None = None
    first_frame: float | None = None
    deadline = start + timeout
    while time.perf_counter() < deadline:
        n = drain_and_reply(master, buf, timeout=0.05)
        now = time.perf_counter()
        if n and first_byte is None:
            first_byte = now - start
        if first_frame is None and looks_like_frame(bytes(buf)):
            first_frame = now - start
            break
        if proc.poll() is not None:
            break
    return first_byte, first_frame, buf


def percentile(xs: list[float], p: float) -> float:
    if not xs:
        return 0.0
    ys = sorted(xs)
    if len(ys) == 1:
        return ys[0]
    k = (len(ys) - 1) * p
    f = int(k)
    c = min(f + 1, len(ys) - 1)
    if f == c:
        return ys[f]
    return ys[f] + (ys[c] - ys[f]) * (k - f)


# ---------------------------------------------------------------------------
# Section A: --version via hyperfine
# ---------------------------------------------------------------------------


def bench_version(agents: list[str], warmup: int, runs: int) -> list[VersionResult]:
    results: list[VersionResult] = []
    hyperfine = shutil.which("hyperfine")
    if not hyperfine:
        print("  ! hyperfine not found — skipping section A (brew install hyperfine)")
        for name in agents:
            results.append(
                VersionResult(agent=name, available=bool(which_agent(name)), error="hyperfine missing")
            )
        return results

    for name in agents:
        path = which_agent(name)
        if not path:
            results.append(VersionResult(agent=name, available=False, error="not on PATH"))
            print(f"  {name:10} SKIP (not on PATH)")
            continue
        argv = AGENT_VERSION_ARGV[name]
        cmd = " ".join(shlex_join(argv))
        try:
            proc = subprocess.run(
                [
                    hyperfine,
                    "--style",
                    "none",
                    "--warmup",
                    str(warmup),
                    "--runs",
                    str(runs),
                    "--export-json",
                    "/dev/stdout",
                    cmd,
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            # hyperfine may print progress on stderr; JSON on stdout
            data = json.loads(proc.stdout)
            entry = data["results"][0]
            vr = VersionResult(
                agent=name,
                available=True,
                mean_ms=entry["mean"] * 1000,
                stddev_ms=entry["stddev"] * 1000,
                min_ms=entry["min"] * 1000,
                max_ms=entry["max"] * 1000,
                runs=entry.get("times") and len(entry["times"]) or runs,
                raw=proc.stdout,
            )
            results.append(vr)
            print(
                f"  {name:10} {vr.mean_ms:7.2f} ms ± {vr.stddev_ms:5.2f}  "
                f"(min {vr.min_ms:.2f}, max {vr.max_ms:.2f}, n={vr.runs})"
            )
        except Exception as e:
            results.append(VersionResult(agent=name, available=True, error=str(e)))
            print(f"  {name:10} ERROR {e}")
    return results


def shlex_join(argv: list[str]) -> list[str]:
    # tiny local join that quotes only when needed
    import shlex

    return [shlex.quote(a) for a in argv]


# ---------------------------------------------------------------------------
# Section B: TUI open / first frame
# ---------------------------------------------------------------------------


def bench_open(agents: list[str], cwd: str, timeout: float, repeats: int) -> list[OpenResult]:
    results: list[OpenResult] = []
    for name in agents:
        if not which_agent(name):
            results.append(OpenResult(agent=name, available=False, error="not on PATH"))
            print(f"  {name:10} SKIP (not on PATH)")
            continue
        argv = AGENT_ARGV[name]
        frames: list[float] = []
        bytes_last = 0
        err: str | None = None
        for _ in range(repeats):
            proc = None
            master = None
            try:
                proc, master = spawn_pty(argv, cwd)
                first_byte, first_frame, buf = wait_first_frame(proc, master, timeout)
                bytes_last = len(buf)
                if first_frame is None:
                    err = f"no frame within {timeout}s (bytes={len(buf)})"
                else:
                    frames.append(first_frame * 1000)
            except Exception as e:
                err = str(e)
            finally:
                if proc is not None:
                    kill_tree(proc)
                if master is not None:
                    try:
                        os.close(master)
                    except OSError:
                        pass
            time.sleep(0.15)
        if frames:
            mean = statistics.mean(frames)
            results.append(
                OpenResult(
                    agent=name,
                    available=True,
                    first_byte_ms=None,
                    first_frame_ms=mean,
                    best_ms=min(frames),
                    worst_ms=max(frames),
                    bytes_seen=bytes_last,
                )
            )
            print(
                f"  {name:10} first_frame {mean:7.1f} ms  "
                f"(best {min(frames):.1f}, worst {max(frames):.1f}, n={len(frames)})"
            )
        else:
            results.append(OpenResult(agent=name, available=True, error=err, bytes_seen=bytes_last))
            print(f"  {name:10} ERROR {err}")
    return results


# ---------------------------------------------------------------------------
# Section C: Idle CPU
# ---------------------------------------------------------------------------


def bench_idle(
    agents: list[str],
    cwd: str,
    open_timeout: float,
    settle_s: float,
    sample_s: float,
    interval_s: float,
) -> list[IdleResult]:
    results: list[IdleResult] = []
    for name in agents:
        if not which_agent(name):
            results.append(IdleResult(agent=name, available=False, error="not on PATH"))
            print(f"  {name:10} SKIP (not on PATH)")
            continue
        argv = AGENT_ARGV[name]
        proc = None
        master = None
        try:
            # Reset linux jiffy baseline between agents
            _linux_prev.clear()
            proc, master = spawn_pty(argv, cwd)
            first_byte, first_frame, buf = wait_first_frame(proc, master, open_timeout)
            if first_frame is None:
                results.append(
                    IdleResult(
                        agent=name,
                        available=True,
                        error=f"no frame within {open_timeout}s (bytes={len(buf)})",
                    )
                )
                print(f"  {name:10} ERROR no first frame (bytes={len(buf)})")
                continue

            # Settle: keep answering queries / draining
            settle_end = time.perf_counter() + settle_s
            while time.perf_counter() < settle_end:
                drain_and_reply(master, buf, timeout=0.1)
                if proc.poll() is not None:
                    break

            if proc.poll() is not None:
                results.append(
                    IdleResult(
                        agent=name,
                        available=True,
                        first_frame_ms=first_frame * 1000,
                        error=f"exited during settle (code={proc.returncode})",
                    )
                )
                print(f"  {name:10} ERROR exited during settle")
                continue

            samples: list[IdleSample] = []
            sample_end = time.perf_counter() + sample_s
            # Prime linux counters once
            pids = descendant_pids(proc.pid)
            read_cpu_rss(pids)
            time.sleep(interval_s)

            while time.perf_counter() < sample_end:
                drain_and_reply(master, buf, timeout=0.0)
                if proc.poll() is not None:
                    break
                pids = descendant_pids(proc.pid)
                cpu, rss, n = read_cpu_rss(pids)
                samples.append(IdleSample(cpu_pct=cpu, rss_kb=rss, nprocs=n))
                time.sleep(interval_s)

            if not samples:
                results.append(
                    IdleResult(
                        agent=name,
                        available=True,
                        first_frame_ms=first_frame * 1000,
                        error="no samples",
                    )
                )
                print(f"  {name:10} ERROR no samples")
                continue

            cpus = [s.cpu_pct for s in samples]
            rsss = [s.rss_kb for s in samples]
            ir = IdleResult(
                agent=name,
                available=True,
                first_frame_ms=first_frame * 1000,
                cpu_mean=statistics.mean(cpus),
                cpu_p50=percentile(cpus, 0.50),
                cpu_p95=percentile(cpus, 0.95),
                cpu_max=max(cpus),
                cpu_min=min(cpus),
                rss_mean_mb=statistics.mean(rsss) / 1024.0,
                nprocs=samples[-1].nprocs,
                samples=len(samples),
            )
            results.append(ir)
            print(
                f"  {name:10} cpu mean={ir.cpu_mean:5.1f}%  "
                f"p50={ir.cpu_p50:5.1f}%  p95={ir.cpu_p95:5.1f}%  "
                f"max={ir.cpu_max:5.1f}%  rss={ir.rss_mean_mb:6.1f}MB  "
                f"procs={ir.nprocs}  n={ir.samples}"
            )
        except Exception as e:
            results.append(IdleResult(agent=name, available=True, error=str(e)))
            print(f"  {name:10} ERROR {e}")
        finally:
            if proc is not None:
                kill_tree(proc)
            if master is not None:
                try:
                    os.close(master)
                except OSError:
                    pass
        time.sleep(0.2)
    return results


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------


def print_summary(report: Report) -> None:
    print()
    print("=" * 72)
    print("SUMMARY")
    print("=" * 72)

    if report.version:
        print()
        print("A) --version (lower is better)")
        rows = [v for v in report.version if v.mean_ms is not None]
        rows.sort(key=lambda v: v.mean_ms or 1e9)
        if rows:
            best = rows[0].mean_ms or 1.0
            for v in rows:
                ratio = (v.mean_ms or 0) / best
                flag = " ←" if v.agent == "crabcode" else ""
                print(f"  {v.agent:10} {v.mean_ms:7.2f} ms   ({ratio:.2f}× best){flag}")

    if report.open:
        print()
        print("B) TUI first frame (lower is better)")
        rows = [o for o in report.open if o.first_frame_ms is not None]
        rows.sort(key=lambda o: o.first_frame_ms or 1e9)
        if rows:
            best = rows[0].first_frame_ms or 1.0
            for o in rows:
                ratio = (o.first_frame_ms or 0) / best
                flag = " ←" if o.agent == "crabcode" else ""
                print(f"  {o.agent:10} {o.first_frame_ms:7.1f} ms   ({ratio:.2f}× best){flag}")

    if report.idle:
        print()
        print("C) Idle CPU after settle (lower is better)")
        rows = [i for i in report.idle if i.cpu_mean is not None]
        # Prefer crabcode on exact ties so the ← marker sits on the winner row.
        rows.sort(key=lambda i: (round(i.cpu_mean or 1e9, 2), 0 if i.agent == "crabcode" else 1))
        if rows:
            best = rows[0].cpu_mean or 0.0
            for i in rows:
                if best < 0.05:
                    ratio_s = "tied" if (i.cpu_mean or 0) < 0.05 else f"+{i.cpu_mean:.1f}pp"
                else:
                    ratio_s = f"{(i.cpu_mean or 0) / best:.2f}× best"
                flag = " ←" if i.agent == "crabcode" else ""
                print(
                    f"  {i.agent:10} {i.cpu_mean:5.1f}% mean   "
                    f"p95={i.cpu_p95:5.1f}%  rss={i.rss_mean_mb:6.1f}MB   "
                    f"({ratio_s}){flag}"
                )

        crab = next((i for i in report.idle if i.agent == "crabcode" and i.cpu_mean is not None), None)
        peers = [i for i in report.idle if i.agent != "crabcode" and i.cpu_mean is not None]
        if crab and peers:
            # Treat <0.05% as floor noise on macOS ps.
            crab_cpu = crab.cpu_mean or 0.0
            winners = [p for p in peers if (p.cpu_mean or 0) + 0.05 < crab_cpu]
            if winners:
                names = ", ".join(p.agent for p in winners)
                print()
                print(f"  verdict: crabcode loses idle-CPU to: {names}")
                print("           aim: mean ≤ best peer (and ≪ 100% on Linux)")
            else:
                print()
                print("  verdict: crabcode best (or tied) on idle CPU ✓")

    print()
    print(f"host={report.host}  platform={report.platform}  cwd={report.cwd}")
    print(f"settle={report.settle_s}s  sample={report.sample_s}s  interval={report.sample_interval_s}s")
    print()
    print("Notes")
    print("  • macOS `ps %cpu` is smoothed — use Linux /proc for a clearer idle peg.")
    print("  • Run from a real project dir to include indexer cost.")
    print("  • Compare release binary: `cargo build --release && PATH=./target/release:$PATH just bench-perf`")


# ---------------------------------------------------------------------------
# PERF.md update
# ---------------------------------------------------------------------------


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def default_perf_md() -> Path:
    return repo_root() / "PERF.md"


def _cwd_label(cwd: str) -> str:
    root = str(repo_root())
    if os.path.abspath(cwd) == root:
        return "repo"
    return cwd


def _idle_verdict(report: Report) -> str:
    crab = next((i for i in report.idle if i.agent == "crabcode" and i.cpu_mean is not None), None)
    peers = [i for i in report.idle if i.agent != "crabcode" and i.cpu_mean is not None]
    if not crab or not peers:
        return "n/a (incomplete idle section)"
    crab_cpu = crab.cpu_mean or 0.0
    winners = [p for p in peers if (p.cpu_mean or 0) + 0.05 < crab_cpu]
    if winners:
        names = " + ".join(p.agent for p in winners)
        return f"loses idle-CPU to {names} on this run. Aim: mean ≤ best peer (and ≪ 100% on Linux)."
    return "crabcode best (or tied) on idle CPU."


def format_latest_markdown(report: Report, version_runs: int | None) -> str:
    today = dt.date.today().isoformat()
    runs = version_runs
    if runs is None:
        for v in report.version:
            if v.runs:
                runs = v.runs
                break
    runs_s = str(runs) if runs is not None else "?"

    lines: list[str] = []
    lines.append(f"**{today}** · {report.platform} · `{report.host}` · cwd = {_cwd_label(report.cwd)}  ")
    lines.append(
        f"settle=`{report.settle_s:g}s` · sample=`{report.sample_s:g}s` · "
        f"interval=`{report.sample_interval_s:g}s` · version runs=`{runs_s}`"
    )
    lines.append("")

    if any(v.mean_ms is not None for v in report.version):
        rows = [v for v in report.version if v.mean_ms is not None]
        rows.sort(key=lambda v: v.mean_ms or 1e9)
        best = rows[0]
        lines.append("### A) `--version` (lower is better)")
        lines.append("")
        lines.append("| Agent | mean ± σ | min … max |")
        lines.append("| --- | ---: | ---: |")
        for v in rows:
            name = f"**{v.agent}**" if v.agent == best.agent else v.agent
            mean = f"**{v.mean_ms:.2f} ms ± {v.stddev_ms:.2f}**" if v.agent == best.agent else f"{v.mean_ms:.2f} ms ± {v.stddev_ms:.2f}"
            lines.append(f"| {name} | {mean} | {v.min_ms:.2f} … {v.max_ms:.2f} |")
        lines.append("")
        crab = next((v for v in rows if v.agent == "crabcode"), None)
        if crab and best.agent == "crabcode" and best.mean_ms:
            parts: list[str] = []
            for v in rows:
                if v.agent == "crabcode" or not v.mean_ms:
                    continue
                ratio = v.mean_ms / best.mean_ms
                if v.agent == "opencode":
                    parts.append(f"**~{ratio:.0f}×** than opencode")
                elif not parts:
                    parts.append(f"**{ratio:.2f}×** faster than {v.agent}")
                else:
                    parts.append(f"**{ratio:.2f}×** than {v.agent}")
            if parts:
                lines.append("crabcode is " + ", ".join(parts) + ".")
                lines.append("")
        elif crab and best.mean_ms and crab.mean_ms:
            lines.append(
                f"crabcode is **{crab.mean_ms / best.mean_ms:.2f}×** best "
                f"(best: {best.agent})."
            )
            lines.append("")

    if any(o.first_frame_ms is not None for o in report.open):
        rows = [o for o in report.open if o.first_frame_ms is not None]
        rows.sort(key=lambda o: o.first_frame_ms or 1e9)
        best = rows[0]
        lines.append("### B) TUI first frame (lower is better)")
        lines.append("")
        lines.append("| Agent | mean | best … worst |")
        lines.append("| --- | ---: | ---: |")
        for o in rows:
            name = f"**{o.agent}**" if o.agent == best.agent else o.agent
            mean = (
                f"**{o.first_frame_ms:.1f} ms**"
                if o.agent == best.agent
                else f"{o.first_frame_ms:.1f} ms"
            )
            if o.best_ms is not None and o.worst_ms is not None:
                rng = f"{o.best_ms:.1f} … {o.worst_ms:.1f}"
            else:
                rng = "—"
            lines.append(f"| {name} | {mean} | {rng} |")
        lines.append("")

    if any(i.cpu_mean is not None for i in report.idle):
        rows = [i for i in report.idle if i.cpu_mean is not None]
        rows.sort(key=lambda i: (round(i.cpu_mean or 1e9, 2), 0 if i.agent == "crabcode" else 1))
        best = rows[0]
        lines.append("### C) Idle CPU after settle (lower is better)")
        lines.append("")
        lines.append("| Agent | mean | p50 | p95 | max | RSS |")
        lines.append("| --- | ---: | ---: | ---: | ---: | ---: |")
        for i in rows:
            name = f"**{i.agent}**" if i.agent == best.agent else i.agent
            mean = f"**{i.cpu_mean:.1f}%**" if i.agent == best.agent else f"{i.cpu_mean:.1f}%"
            lines.append(
                f"| {name} | {mean} | {i.cpu_p50:.1f}% | {i.cpu_p95:.1f}% | "
                f"{i.cpu_max:.1f}% | {i.rss_mean_mb:.1f} MB |"
            )
        lines.append("")
        lines.append(f"**Verdict:** {_idle_verdict(report)}")
        lines.append("")

    lines.append(
        "> Tip: use a **release** binary and `--settle 5 --sample 10+`. "
        "Debug builds / short settle can still show Home blink (~60fps) and inflate idle %."
    )
    lines.append("")
    lines.append("<details>")
    lines.append("<summary>Raw dump</summary>")
    lines.append("")
    lines.append("```")
    lines.extend(_raw_dump_lines(report))
    lines.append("```")
    lines.append("")
    lines.append("</details>")
    return "\n".join(lines)


def _raw_dump_lines(report: Report) -> list[str]:
    out: list[str] = []
    if report.version:
        out.append("A) --version startup (hyperfine)")
        for v in report.version:
            if v.mean_ms is None:
                out.append(f"  {v.agent:10} ERROR {v.error or 'n/a'}")
            else:
                out.append(
                    f"  {v.agent:10} {v.mean_ms:6.2f} ms ± {v.stddev_ms:5.2f}  "
                    f"(min {v.min_ms:.2f}, max {v.max_ms:.2f}, n={v.runs})"
                )
        out.append("")
    if report.open:
        out.append("B) TUI first frame")
        for o in report.open:
            if o.first_frame_ms is None:
                out.append(f"  {o.agent:10} ERROR {o.error or 'n/a'}")
            elif o.best_ms is not None and o.worst_ms is not None:
                out.append(
                    f"  {o.agent:10} first_frame {o.first_frame_ms:7.1f} ms  "
                    f"(best {o.best_ms:.1f}, worst {o.worst_ms:.1f})"
                )
            else:
                out.append(f"  {o.agent:10} first_frame {o.first_frame_ms:7.1f} ms")
        out.append("")
    if report.idle:
        out.append(
            f"C) Idle CPU (settle={report.settle_s:g}s, sample={report.sample_s:g}s)"
        )
        for i in report.idle:
            if i.cpu_mean is None:
                out.append(f"  {i.agent:10} ERROR {i.error or 'n/a'}")
            else:
                out.append(
                    f"  {i.agent:10} cpu mean={i.cpu_mean:5.1f}%  "
                    f"p50={i.cpu_p50:5.1f}%  p95={i.cpu_p95:5.1f}%  "
                    f"max={i.cpu_max:5.1f}%  rss={i.rss_mean_mb:6.1f}MB  "
                    f"procs={i.nprocs}  n={i.samples}"
                )
    return out


def update_perf_md(path: Path, report: Report, version_runs: int | None) -> None:
    if not path.exists():
        raise FileNotFoundError(f"{path} not found")

    text = path.read_text()
    latest_new = format_latest_markdown(report, version_runs=version_runs)

    # Split on ## Latest ... ## History
    m = re.search(
        r"(?s)(## Latest\n)(.*?)(\n---\n\n## History\n)(.*?)(\n---\n\n## How to refresh\n)",
        text,
    )
    if not m:
        raise ValueError(
            f"{path} missing expected ## Latest / ## History / ## How to refresh markers"
        )

    old_latest = m.group(2).strip("\n")
    old_history = m.group(4).strip("\n")

    # Archive previous Latest into History (newest first), skip placeholder
    placeholder = old_latest.strip().startswith("_(") or "none yet" in old_latest.lower()
    archived = []
    if old_latest.strip() and not placeholder:
        archived.append("<details>")
        archived.append(f"<summary>{_history_summary(old_latest)}</summary>")
        archived.append("")
        archived.append(old_latest.strip())
        archived.append("")
        archived.append("</details>")
    if old_history.strip() and "none yet" not in old_history.lower():
        archived.append("")
        archived.append(old_history.strip())

    history_body = "\n".join(archived).strip() if archived else "_(none yet)_"

    new_text = (
        text[: m.start()]
        + m.group(1)
        + "\n"
        + latest_new
        + "\n"
        + m.group(3)
        + "\n"
        + history_body
        + "\n"
        + m.group(5)
        + text[m.end() :]
    )
    path.write_text(new_text)


def _history_summary(latest_block: str) -> str:
    first = latest_block.strip().splitlines()[0] if latest_block.strip() else "previous run"
    # Strip markdown bold
    return first.replace("**", "").strip()


def prompt_write_perf(report: Report, version_runs: int | None, perf_path: Path) -> None:
    if not sys.stdin.isatty():
        print("\n(non-interactive — skip PERF.md prompt; pass --write-perf to update)")
        return
    try:
        ans = input("\nAdd this to PERF.md? [y/N] ").strip().lower()
    except EOFError:
        return
    if ans not in ("y", "yes"):
        print("skipped")
        return
    update_perf_md(perf_path, report, version_runs=version_runs)
    print(f"updated {perf_path}")


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument(
        "--agents",
        default=",".join(DEFAULT_AGENTS),
        help=f"comma-separated agents (default: {','.join(DEFAULT_AGENTS)})",
    )
    p.add_argument(
        "--section",
        choices=("all", "version", "open", "idle"),
        default="all",
        help="which section to run",
    )
    p.add_argument("--cwd", default=os.getcwd(), help="working directory for TUI launch")
    p.add_argument("--settle", type=float, default=3.0, help="seconds to settle before idle sample")
    p.add_argument("--sample", type=float, default=8.0, help="seconds of idle CPU sampling")
    p.add_argument("--interval", type=float, default=0.25, help="sample interval seconds")
    p.add_argument("--open-timeout", type=float, default=8.0, help="max wait for first frame")
    p.add_argument("--open-repeats", type=int, default=3, help="TUI open repeats")
    p.add_argument("--warmup", type=int, default=5, help="hyperfine warmup runs")
    p.add_argument("--runs", type=int, default=50, help="hyperfine measured runs")
    p.add_argument("--json-out", default=None, help="write full report JSON to path")
    p.add_argument(
        "--perf-md",
        default=None,
        help="PERF.md path (default: repo PERF.md)",
    )
    g = p.add_mutually_exclusive_group()
    g.add_argument(
        "--write-perf",
        action="store_true",
        help="write results into PERF.md without prompting",
    )
    g.add_argument(
        "--no-write-perf",
        action="store_true",
        help="skip the Add-to-PERF.md prompt",
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()
    agents = [a.strip() for a in args.agents.split(",") if a.strip()]
    unknown = [a for a in agents if a not in AGENT_ARGV]
    if unknown:
        print(f"unknown agents: {unknown}. known: {list(AGENT_ARGV)}", file=sys.stderr)
        return 2

    import socket

    report = Report(
        host=socket.gethostname(),
        platform=sys.platform,
        cwd=os.path.abspath(args.cwd),
        settle_s=args.settle,
        sample_s=args.sample,
        sample_interval_s=args.interval,
    )

    print(f"bench-perf  agents={agents}  cwd={report.cwd}  platform={report.platform}")
    print()

    do_all = args.section == "all"

    if do_all or args.section == "version":
        print("A) --version startup (hyperfine)")
        report.version = bench_version(agents, warmup=args.warmup, runs=args.runs)
        print()

    if do_all or args.section == "open":
        print("B) TUI first frame")
        report.open = bench_open(
            agents, cwd=report.cwd, timeout=args.open_timeout, repeats=args.open_repeats
        )
        print()

    if do_all or args.section == "idle":
        print(f"C) Idle CPU (settle={args.settle}s, sample={args.sample}s)")
        report.idle = bench_idle(
            agents,
            cwd=report.cwd,
            open_timeout=args.open_timeout,
            settle_s=args.settle,
            sample_s=args.sample,
            interval_s=args.interval,
        )

    print_summary(report)

    if args.json_out:
        Path(args.json_out).write_text(json.dumps(asdict(report), indent=2) + "\n")
        print(f"wrote {args.json_out}")

    perf_path = Path(args.perf_md) if args.perf_md else default_perf_md()
    if args.write_perf:
        update_perf_md(perf_path, report, version_runs=args.runs)
        print(f"updated {perf_path}")
    elif not args.no_write_perf:
        prompt_write_perf(report, version_runs=args.runs, perf_path=perf_path)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
