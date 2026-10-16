import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

// Always exercise the compiled workspace CLI, never a crabcode on the host PATH.
// Build first: cargo build; bun test scripts/upgrade-ui.test.ts
const cli = resolve(import.meta.dir, "../target/debug/crabcode");
const current = /^version = "([^"]+)"/m.exec(
  readFileSync(resolve(import.meta.dir, "../Cargo.toml"), "utf8"),
)![1];
const target = "1.2.3";
const oldBinary = `#!/bin/sh\nprintf 'crabcode ${current}\\n'\n`;
const newBinary = `#!/bin/sh\nprintf 'crabcode ${target}\\n'\n`;
const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});
const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;
const stripAnsi = (s: string) => s.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");

function fixture(
  layout: "default" | "custom" | "cargo-home" = "default",
  drift = false,
) {
  if (!existsSync(cli))
    throw new Error(
      `Missing compiled CLI: ${cli}; run cargo build before this suite (no PATH fallback).`,
    );
  const dir = realpathSync(
    mkdtempSync(join(tmpdir(), "crabcode-upgrade-test-")),
  );
  dirs.push(dir);
  const home = join(dir, "home");
  const tools = join(dir, "tools");
  const cargo = join(home, ".cargo");
  const prefix =
    layout === "custom"
      ? join(home, "custom-flat")
      : layout === "cargo-home"
        ? cargo
        : join(home, ".local/bin");
  const installed = join(
    prefix,
    ...(layout === "cargo-home" ? ["bin"] : []),
    "crabcode",
  );
  const config = join(home, ".config");
  for (const path of [
    tools,
    dirname(installed),
    join(config, "crabcode"),
    join(dir, "payload"),
  ])
    mkdirSync(path, { recursive: true });
  writeFileSync(installed, oldBinary, { mode: 0o755 });
  writeFileSync(join(dir, "payload/crabcode"), newBinary, { mode: 0o755 });
  const archive = join(dir, "archive.tar.gz");
  const tar = spawnSync(
    "/usr/bin/tar",
    ["-czf", archive, "-C", join(dir, "payload"), "crabcode"],
    { encoding: "utf8" },
  );
  if (tar.status !== 0) throw new Error(`fixture tar failed: ${tar.stderr}`);
  writeFileSync(
    `${archive}.sha256`,
    `${createHash("sha256").update(readFileSync(archive)).digest("hex")}  archive.tar.gz\n`,
  );
  function receipt(at: string, kind: string) {
    writeFileSync(
      join(config, "crabcode/crabcode-receipt.json"),
      JSON.stringify({
        binaries: ["crabcode"],
        binary_aliases: {},
        install_prefix: at,
        install_layout: kind,
      }),
    );
  }
  if (layout !== "default")
    receipt(prefix, layout === "custom" ? "flat" : "cargo-home");
  function tool(name: string, body: string) {
    writeFileSync(join(tools, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  }
  // No host package manager is allowed to install anything. Historical receipt
  // detection can query cargo --list, but every mutating cargo command fails.
  tool(
    "cargo",
    '[ "$*" = "install --list" ] && exit 0; echo "unexpected cargo: $*" >&2; exit 99',
  );
  tool(
    "curl",
    `
set -eu
printf '%s\\n' "$*" >>"$FIXTURE/curl.log"
output= url= writeout=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o|--output) output=$2; shift 2 ;;
    --write-out) writeout=1; shift 2 ;;
    --proto|--proto-redir|--connect-timeout|--max-time|-H) shift 2 ;;
    -*) shift ;;
    *) url=$1; shift ;;
  esac
done
case "$url" in
  */releases/download/v1.2.3/*) ;;
  *) echo "unexpected or unpinned URL: $url" >&2; exit 99 ;;
esac
if [ "\u0024{MODE:-}" = failure ]; then
  [ -z "$output" ] || printf partial >"$output"
  [ "$writeout" = 0 ] || printf 500
  echo 'curl: (22) HTTP 500 fixture failure' >&2
  exit 22
fi
case "$url" in
  */crabcode-installer.sh) source="$FIXTURE/dist-installer.sh" ;;
  *.tar.gz.sha256) source="$FIXTURE/archive.tar.gz.sha256" ;;
  *.tar.gz) sleep 0.3; source="$FIXTURE/archive.tar.gz" ;;
  *) echo "unexpected asset: $url" >&2; exit 99 ;;
esac
if [ -n "$output" ]; then cp "$source" "$output"; else cat "$source"; fi
[ "$writeout" = 0 ] || printf 200
`,
  );
  writeFileSync(
    join(dir, "dist-installer.sh"),
    `#!/bin/sh
set -eu
[ "$#" = 0 ] || { echo 'unexpected installer arguments' >&2; exit 91; }
[ "$CRABCODE_INSTALL_DIR" = "$EXPECTED_PREFIX" ] || exit 92
[ "$CARGO_DIST_FORCE_INSTALL_DIR" = "$EXPECTED_PREFIX" ] || exit 93
[ "$CARGO_HOME" = "$EXPECTED_CARGO_HOME" ] || exit 94
mkdir -p "$EXPECTED_PREFIX/bin"
cp "$FIXTURE/payload/crabcode" "$EXPECTED_PREFIX/bin/crabcode"
printf 'dist installer output\\n'
`,
    { mode: 0o755 },
  );
  const cargoHome = drift ? join(home, "drifted-cargo") : cargo;
  // Deliberately do not inherit CRABCODE_INSTALL_DIR, NO_COLOR, or host PATH.
  const env = {
    HOME: home,
    XDG_CONFIG_HOME: config,
    CARGO_HOME: cargoHome,
    PATH: `${dirname(installed)}:${tools}:/usr/bin:/bin`,
    FIXTURE: dir,
    EXPECTED_PREFIX: prefix,
    EXPECTED_CARGO_HOME: cargoHome,
    TMPDIR: dir,
    TERM: "xterm",
    MODE: "",
  };
  function run(
    version = target,
    overrides: Record<string, string> = {},
    columns?: number,
  ) {
    const fullEnv = { ...env, ...overrides };
    let executable = cli;
    let args = ["upgrade", version];
    if (columns !== undefined) {
      executable = "/usr/bin/script";
      // Disable the PTY line discipline's LF->CRLF translation so CR assertions
      // measure CLI rendering, rather than script's terminal transport.
      const shell = `stty rows 24 cols ${columns} -onlcr; exec ${quote(cli)} upgrade ${quote(version)} </dev/null`;
      args =
        process.platform === "darwin"
          ? ["-q", "/dev/null", "/bin/sh", "-c", shell]
          : ["-q", "-e", "-c", shell, "/dev/null"];
    }
    const result = spawnSync(executable, args, {
      env: fullEnv,
      encoding: "utf8",
      timeout: 20000,
    });
    if (result.error) throw result.error;
    return {
      ...result,
      text:
        columns === undefined ? result.stderr : result.stdout + result.stderr,
    };
  }
  const log = () =>
    existsSync(join(dir, "curl.log"))
      ? readFileSync(join(dir, "curl.log"), "utf8")
      : "";
  return { dir, home, prefix, installed, cargoHome, receipt, run, log };
}

function successful(
  f: ReturnType<typeof fixture>,
  result: ReturnType<ReturnType<typeof fixture>["run"]>,
) {
  expect(result.status).toBe(0);
  expect(result.text).toContain(`Installed version: ${current}`);
  expect(result.text).toContain(`Upgrading crabcode version: ${target}`);
  expect(result.text).toContain("Using install.sh");
  expect(result.text).toContain(
    `Upgraded crabcode to ${target}. Restart crabcode to apply.`,
  );
  expect(readFileSync(f.installed, "utf8")).toBe(newBinary);
  expect(existsSync(join(dirname(f.installed), "bin/crabcode"))).toBe(false);
  expect(f.log()).not.toMatch(/releases\/latest|api\.github/);
}

describe("compiled CLI upgrade UI and install routing (offline)", () => {
  test("redirected UI is stderr only and installs the pinned archive in the active flat directory", () => {
    const f = fixture();
    const r = f.run("v1.2.3");
    successful(f, r);
    expect(r.stdout).toBe("");
    expect(r.text).toContain("Working...");
    expect(r.text).not.toMatch(/[\x1b\r%]/);
    expect(f.log()).toContain("/releases/download/v1.2.3/crabcode-");
    expect(f.log()).toContain(".tar.gz.sha256");
  });
  test("HTTP 500 preserves the executable and reports failure without success or fallback", () => {
    const f = fixture();
    const r = f.run(target, { MODE: "failure" });
    expect(r.status).not.toBe(0);
    expect(r.stdout).toBe("");
    expect(r.text).toContain("HTTP 500 fixture failure");
    expect(r.text).not.toContain("Upgraded crabcode to");
    expect(r.text).not.toMatch(/[\x1b\r]/);
    expect(readFileSync(f.installed, "utf8")).toBe(oldBinary);
    expect(f.log()).not.toMatch(/latest|tar\.xz/);
  });
  test("a stale receipt cannot redirect the active ~/.local/bin installation", () => {
    const f = fixture();
    const other = join(f.home, "other-prefix");
    mkdirSync(other);
    writeFileSync(join(other, "crabcode"), "untouched");
    f.receipt(other, "flat");
    successful(f, f.run());
    expect(readFileSync(join(other, "crabcode"), "utf8")).toBe("untouched");
    expect(existsSync(join(other, "bin"))).toBe(false);
  });
  test("a matching custom flat receipt installs in place, not prefix/bin/bin", () => {
    const f = fixture("custom");
    successful(f, f.run());
  });
  for (const drift of [false, true]) {
    test(`historical cargo-home receipt uses the tagged dist installer with no arguments (drift=${drift})`, () => {
      const f = fixture("cargo-home", drift);
      const r = f.run();
      successful(f, r);
      expect(r.stdout).toBe("");
      expect(f.log().trim().split("\n")).toHaveLength(1);
      expect(f.log()).toContain(
        "/releases/download/v1.2.3/crabcode-installer.sh",
      );
      if (drift)
        expect(existsSync(join(f.cargoHome, "bin/crabcode"))).toBe(false);
    });
  }
  test("same-version upgrade is a no-op and never invokes curl", () => {
    const f = fixture();
    const r = f.run(current);
    expect(r.status).toBe(0);
    expect(r.stdout).toBe("");
    expect(r.text).toContain(`Already on ${current}. Nothing to upgrade.`);
    expect(r.text).not.toContain("Working");
    expect(f.log()).toBe("");
    expect(readFileSync(f.installed, "utf8")).toBe(oldBinary);
  });
  test("invalid version never runs an installer or curl", () => {
    const f = fixture();
    const r = f.run("not-a-version");
    expect(r.status).not.toBe(0);
    expect(r.stdout).toBe("");
    expect(r.text).toContain("invalid upgrade version");
    expect(f.log()).toBe("");
    expect(readFileSync(f.installed, "utf8")).toBe(oldBinary);
  });
  for (const columns of [20, 40, 80]) {
    test(`PTY ${columns} columns renders amber Working without fabricated percentages or wrapping`, () => {
      const f = fixture();
      const r = f.run(target, {}, columns);
      successful(f, r);
      expect(r.text).toContain("\x1b[33m");
      expect(r.text).toContain(" Working");
      expect(r.text).not.toMatch(/\d+(?:\.\d+)?%/);
      const rows = [...r.text.matchAll(/\r\x1b\[2K([^\r\n]*)/g)]
        .map((m) => stripAnsi(m[1]))
        .filter((row) => row.includes("Working"));
      expect(rows.length).toBeGreaterThan(0);
      for (const row of rows)
        expect([...row].length).toBeLessThanOrEqual(columns - 1);
    });
  }
  test("PTY NO_COLOR suppresses SGR colors while retaining clear-line rendering", () => {
    const f = fixture();
    const r = f.run(target, { NO_COLOR: "" }, 40);
    successful(f, r);
    expect(r.text).not.toMatch(/\x1b\[[0-9;]*m/);
    expect(r.text).toContain("\x1b[2K");
    expect(r.text).toContain(" Working");
    expect(r.text).not.toContain("%");
  });
  test("PTY TERM=dumb has neither ANSI nor carriage-return rendering", () => {
    const f = fixture();
    const r = f.run(target, { TERM: "dumb" }, 40);
    successful(f, r);
    expect(r.text).toContain("Working...");
    expect(r.text).not.toMatch(/[\x1b\r%]/);
  });
});
