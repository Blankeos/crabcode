import { afterEach, describe, expect, test } from "bun:test";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
  existsSync,
  realpathSync,
  symlinkSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";

const installer = resolve(import.meta.dir, "../install.sh");
const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "crabcode-install-test-"));
  dirs.push(dir);
  const home = join(dir, "home");
  const tools = join(dir, "tools");
  const payload = join(dir, "payload/crabcode-x86_64-unknown-linux-gnu");
  const prefix = join(home, ".local/bin");
  for (const path of [home, tools, payload, prefix])
    mkdirSync(path, { recursive: true });
  const binary = "#!/bin/sh\nprintf 'crabcode 1.2.3\\n'\n";
  writeFileSync(join(payload, "crabcode"), binary, { mode: 0o755 });
  for (const [format, flag] of [
    ["gz", "-czf"],
    ["xz", "-cJf"],
  ]) {
    const archive = join(dir, `archive.tar.${format}`);
    const result = spawnSync("tar", [
      flag,
      archive,
      "-C",
      join(dir, "payload"),
      "crabcode-x86_64-unknown-linux-gnu",
    ]);
    if (result.status !== 0) throw new Error(result.stderr.toString());
    writeFileSync(
      `${archive}.sha256`,
      `${createHash("sha256").update(readFileSync(archive)).digest("hex")}  archive.tar.${format}\n`,
    );
  }
  function tool(name: string, source: string) {
    writeFileSync(join(tools, name), `#!/bin/sh\n${source}\n`, { mode: 0o755 });
  }
  tool(
    "uname",
    'case "$1" in -s) echo "${FAKE_OS:-Linux}" ;; -m) echo "${FAKE_ARCH:-x86_64}" ;; esac',
  );
  tool(
    "curl",
    `
printf '%s\\n' "$*" >>"$FIXTURE/curl.log"
output= progress=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output=$2; shift 2 ;;
    --write-out|--proto|--proto-redir|--connect-timeout|--max-time) shift 2 ;;
    --progress-bar) progress=1; shift ;;
    --*) shift ;;
    *) url=$1; shift ;;
  esac
done
case "$url" in
  */releases/latest) : >"$output"; printf 'https://github.com/blankeos/crabcode/releases/tag/v1.2.3'; exit 0 ;;
esac
if [ "$progress" = 1 ] && [ "\u0024{UNKNOWN_SIZE:-0}" != 1 ]; then
  printf '\\r######## 25.0%%\\r################ 100.0%%\\r' >&2
fi
case "$url" in
  *.tar.gz) [ "\u0024{MODE:-}" != fallback ] || { printf 404; echo 'curl: (22) 404' >&2; exit 22; } ;;
esac
[ "\u0024{MODE:-}" != failure ] || { echo partial >"$output"; printf 500; echo 'curl: (22) server failed' >&2; exit 22; }
case "$url" in
  *.tar.gz.sha256) source="$FIXTURE/archive.tar.gz.sha256" ;;
  *.tar.xz.sha256) source="$FIXTURE/archive.tar.xz.sha256" ;;
  *.tar.gz) source="$FIXTURE/archive.tar.gz" ;;
  *.tar.xz) source="$FIXTURE/archive.tar.xz" ;;
  *) echo 'unexpected URL' >&2; exit 99 ;;
esac
case "$url" in *.sha256)
  [ "\u0024{MODE:-}" != missing-checksum ] || { printf 404; exit 22; }
  [ "\u0024{MODE:-}" != checksum ] || { printf '%064d\\n' 0 >"$output"; printf 200; exit 0; } ;;
esac
cp "$source" "$output"
printf 200
`,
  );
  const env = {
    ...process.env,
    HOME: home,
    XDG_CONFIG_HOME: join(home, ".config"),
    PATH: `${tools}:/usr/bin:/bin:/usr/sbin:/sbin`,
    FIXTURE: dir,
    TERM: "xterm",
    MODE: "",
    CRABCODE_INSTALL_DIR: prefix,
  };
  function run(
    args: string[] = ["1.2.3"],
    overrides: Record<string, string | undefined> = {},
    tty = false,
    input?: string,
  ) {
    const fullEnv = { ...env, ...overrides };
    if (tty) {
      // script provides a real PTY; platform flags differ between BSD and util-linux.
      const shell = `${fullEnv.PTY_COLUMNS ? `stty rows 24 cols ${Number(fullEnv.PTY_COLUMNS)}; ` : ""}/bin/sh '${installer}' ${args.map((a) => `'${a}'`).join(" ")} </dev/null`;
      const command =
        process.platform === "darwin"
          ? ["-q", "/dev/null", "/bin/sh", "-c", shell]
          : ["-q", "-e", "-c", shell, "/dev/null"];
      const result = spawnSync("/usr/bin/script", command, {
        env: fullEnv,
        encoding: "utf8",
        timeout: 15000,
      });
      return { ...result, text: result.stdout + result.stderr };
    }
    const result = spawnSync("/bin/sh", [installer, ...args], {
      env: fullEnv,
      encoding: "utf8",
      timeout: 15000,
      input,
    });
    return { ...result, text: result.stderr };
  }
  function old() {
    writeFileSync(
      join(prefix, "crabcode"),
      "#!/bin/sh\necho 'crabcode 0.9.0'\n",
      { mode: 0o755 },
    );
  }
  return {
    dir,
    home,
    prefix,
    binary,
    run,
    old,
    log: () => readFileSync(join(dir, "curl.log"), "utf8"),
  };
}

describe("POSIX prebuilt installer", () => {
  test("sh syntax is valid", () => {
    expect(spawnSync("/bin/sh", ["-n", installer]).status).toBe(0);
  });
  test("pinned release installs gzip archive and an escaped flat receipt, normal output is stderr", () => {
    const f = fixture();
    const prefix = join(f.home, 'custom "quoted" \\prefix');
    const result = f.run(["v1.2.3"], {
      CRABCODE_INSTALL_DIR: prefix,
      XDG_CONFIG_HOME: join(f.home, "xdg"),
    });
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("");
    expect(result.text).toContain(
      "Installed version: not installed\n\nInstalling crabcode version: 1.2.3",
    );
    expect(result.text).not.toMatch(/[\x1b\r]/);
    expect(readFileSync(join(prefix, "crabcode"), "utf8")).toBe(f.binary);
    const receipt = JSON.parse(
      readFileSync(join(f.home, "xdg/crabcode/crabcode-receipt.json"), "utf8"),
    );
    expect(receipt).toMatchObject({
      binaries: ["crabcode"],
      install_prefix: join(realpathSync(f.home), 'custom "quoted" \\prefix'),
      install_layout: "flat",
    });
    expect(f.log()).toContain(
      "/v1.2.3/crabcode-x86_64-unknown-linux-gnu.tar.gz",
    );
    expect(f.log()).toContain(".tar.gz.sha256");
    expect(f.log()).not.toContain("releases/latest");
    for (const line of f.log().trim().split("\n")) {
      expect(line).toContain("--fail --location");
      expect(line).toContain("--proto =https --proto-redir =https");
      expect(line).toContain("--connect-timeout 10");
      expect(line).toContain(
        line.includes(".sha256") ? "--max-time 20" : "--max-time 300",
      );
    }
  });
  test("defaults to latest, reports the current version and upgrades", () => {
    const f = fixture();
    f.old();
    const result = f.run([]);
    expect(result.status).toBe(0);
    expect(result.text).toContain(
      "Installed version: 0.9.0\n\nInstalling crabcode version: 1.2.3",
    );
    expect(f.log()).toContain("releases/latest");
    expect(
      f
        .log()
        .split("\n")
        .find((line) => line.includes("releases/latest")),
    ).toContain("--connect-timeout 10 --max-time 20");
    expect(readFileSync(join(f.prefix, "crabcode"), "utf8")).toBe(f.binary);
  });
  test("default install prefix is ~/.local/bin", () => {
    const f = fixture();
    expect(f.run(["1.2.3"], { CRABCODE_INSTALL_DIR: "" }).status).toBe(0);
    expect(existsSync(join(f.prefix, "crabcode"))).toBe(true);
  });
  for (const mode of ["checksum", "failure", "missing-checksum"]) {
    test(`${mode} failure preserves installed executable`, () => {
      const f = fixture();
      f.old();
      const before = readFileSync(join(f.prefix, "crabcode"));
      const result = f.run(["1.2.3"], { MODE: mode });
      expect(result.status).not.toBe(0);
      expect(result.text).toContain("Error:");
      expect(readFileSync(join(f.prefix, "crabcode"))).toEqual(before);
      expect(
        existsSync(join(f.home, ".config/crabcode/crabcode-receipt.json")),
      ).toBe(false);
      if (mode === "failure") {
        expect(f.log()).not.toContain("tar.xz");
        expect(result.text).toContain("server failed");
      }
    });
  }
  test("only archive 404 triggers legacy xz and verifies its checksum", () => {
    const f = fixture();
    expect(f.run(["1.2.3"], { MODE: "fallback" }).status).toBe(0);
    expect(f.log()).toContain(".tar.xz.sha256");
    expect(readFileSync(join(f.prefix, "crabcode"), "utf8")).toBe(f.binary);
  });
  for (const [os, arch, target] of [
    ["Darwin", "arm64", "aarch64-apple-darwin"],
    ["Darwin", "aarch64", "aarch64-apple-darwin"],
    ["Darwin", "x86_64", "x86_64-apple-darwin"],
    ["Linux", "aarch64", "aarch64-unknown-linux-gnu"],
  ]) {
    test(`${os}/${arch} selects Rust triple`, () => {
      const f = fixture();
      expect(f.run(["1.2.3"], { FAKE_OS: os, FAKE_ARCH: arch }).status).toBe(0);
      expect(f.log()).toContain(`crabcode-${target}.tar.gz`);
    });
  }
  test("unsupported platform fails without networking", () => {
    const f = fixture();
    expect(f.run([], { FAKE_OS: "FreeBSD" }).status).not.toBe(0);
    expect(f.run([], { FAKE_ARCH: "riscv64" }).status).not.toBe(0);
    expect(existsSync(join(f.dir, "curl.log"))).toBe(false);
  });
  for (const args of [
    [""],
    ["../bad"],
    ["--help"],
    ["1.2"],
    ["1.2.3", "extra"],
    ["1.2.3\nmalicious"],
  ]) {
    test(`rejects arguments ${JSON.stringify(args)}`, () => {
      const f = fixture();
      expect(f.run(args).status).not.toBe(0);
      expect(existsSync(join(f.dir, "curl.log"))).toBe(false);
    });
  }
  test("verified archive symlink is rejected without replacing the old binary", () => {
    const f = fixture();
    f.old();
    const path = join(
      f.dir,
      "payload/crabcode-x86_64-unknown-linux-gnu/crabcode",
    );
    rmSync(path);
    symlinkSync("/etc/passwd", path);
    const archive = join(f.dir, "archive.tar.gz");
    expect(
      spawnSync("tar", [
        "-czf",
        archive,
        "-C",
        join(f.dir, "payload"),
        "crabcode-x86_64-unknown-linux-gnu",
      ]).status,
    ).toBe(0);
    writeFileSync(
      `${archive}.sha256`,
      createHash("sha256").update(readFileSync(archive)).digest("hex") + "\n",
    );
    const result = f.run();
    expect(result.status).not.toBe(0);
    expect(result.text).toContain("not a regular file");
    expect(readFileSync(join(f.prefix, "crabcode"), "utf8")).toContain("0.9.0");
    expect(readdirSync(f.prefix)).toEqual(["crabcode"]);
  });
  test("TTY renders real percentages, amber solid blocks, capped width", () => {
    const f = fixture();
    const result = f.run(["1.2.3"], { COLUMNS: "200" }, true);
    expect(result.status).toBe(0);
    expect(result.text).toContain("\x1b[33m");
    expect(result.text).toContain(
      "█".repeat(12) + ".".repeat(38) + "  25%\x1b[0m",
    );
    expect(result.text).toContain("100%");
    expect(result.text).not.toMatch(
      /\x1b\[\?25|\x1b\[\?1049|\x1b\[2J|\x1b\[2m/,
    );
    expect(result.text.match(/100%/g)).toHaveLength(1);
    expect(result.text).toContain("Verifying download...");
    expect(result.text.indexOf("100%")).toBeLessThan(
      result.text.indexOf("Installed crabcode 1.2.3"),
    );
    expect(
      f
        .log()
        .split("\n")
        .filter((line) => line.includes("--progress-bar")),
    ).toHaveLength(1);
  });
  test("NO_COLOR retains narrow TTY progress without ANSI", () => {
    const f = fixture();
    const result = f.run(["1.2.3"], { NO_COLOR: "", COLUMNS: "16" }, true);
    expect(result.status).toBe(0);
    expect(result.text).not.toContain("\x1b");
    expect(result.text).toContain("██.......  25%");
  });
  for (let columns = 1; columns <= 12; columns++) {
    for (const actualSize of [false, true]) {
      test(`PTY progress fits ${columns} columns (${actualSize ? "stty" : "COLUMNS"})`, () => {
        const f = fixture();
        const result = f.run(
          ["1.2.3"],
          {
            NO_COLOR: "",
            COLUMNS: actualSize ? undefined : String(columns),
            PTY_COLUMNS: String(columns),
          },
          true,
        );
        expect(result.status).toBe(0);
        // Isolate CR redraws from normal phase/status lines and PTY CRLF.
        const redraws = [...result.text.matchAll(/\r([^\r\n]+)(?=\r)/g)].map(
          (match) => match[1],
        );
        if (columns === 1) expect(redraws).toHaveLength(0);
        else {
          expect(redraws.length).toBeGreaterThan(0);
          for (const redraw of redraws)
            expect([...redraw].length).toBeLessThanOrEqual(columns - 1);
        }
      });
    }
  }
  test("absent COLUMNS uses actual stderr terminal width", () => {
    const f = fixture();
    const result = f.run(
      ["1.2.3"],
      { COLUMNS: undefined, PTY_COLUMNS: "24", NO_COLOR: "" },
      true,
    );
    expect(result.status).toBe(0);
    expect(result.text).toContain("█".repeat(4) + ".".repeat(13) + "  25%");
  });
  test("installed version probe is bounded to two seconds, even for a hanging child", () => {
    const f = fixture();
    writeFileSync(join(f.prefix, "crabcode"), "#!/bin/sh\nexec sleep 30\n", {
      mode: 0o755,
    });
    const start = Date.now();
    const result = f.run();
    const elapsed = Date.now() - start;
    expect(result.status).toBe(0);
    expect(result.text).toContain("Installed version: unknown");
    expect(elapsed).toBeGreaterThanOrEqual(1900);
    expect(elapsed).toBeLessThan(5000);
  });
  test("installed version probe cannot consume piped installer stdin", () => {
    const f = fixture();
    writeFileSync(
      join(f.prefix, "crabcode"),
      `#!/bin/sh
if read -r line; then printf '%s' "$line" >"$FIXTURE/consumed"; fi
printf 'crabcode 0.9.0\\n'
`,
      { mode: 0o755 },
    );
    const result = f.run(["1.2.3"], {}, false, "sentinel input\n");
    expect(result.status).toBe(0);
    expect(result.text).toContain("Installed version: 0.9.0");
    expect(existsSync(join(f.dir, "consumed"))).toBe(false);
  });
  for (const output of [
    "\x1b[33mcrabcode 0.9.0\x1b[0m\n",
    "crabcode 0.9.0\ncrabcode 9.9.9\n",
    "crabcode 0.9.0\n\n",
    "crabcode not-a-version\n",
  ]) {
    test(`rejects untrusted installed version ${JSON.stringify(output)}`, () => {
      const f = fixture();
      writeFileSync(join(f.dir, "version-output"), output);
      writeFileSync(
        join(f.prefix, "crabcode"),
        '#!/bin/sh\ncat "$FIXTURE/version-output"\n',
        { mode: 0o755 },
      );
      const result = f.run();
      expect(result.status).toBe(0);
      expect(result.text).toContain("Installed version: unknown");
      expect(result.text).not.toContain("\x1b");
      expect(result.text).not.toContain("9.9.9");
    });
  }
  test("TERM=dumb stays textual even on a TTY", () => {
    const f = fixture();
    const result = f.run(["1.2.3"], { TERM: "dumb" }, true);
    expect(result.status).toBe(0);
    expect(result.text).not.toContain("\x1b");
    expect(result.text).not.toContain("25.0%");
    expect(f.log()).toContain("--silent");
  });
  test("unknown size does not invent progress", () => {
    const f = fixture();
    const result = f.run(["1.2.3"], { UNKNOWN_SIZE: "1" }, true);
    expect(result.status).toBe(0);
    expect(result.text).toContain("Downloading...");
    expect(result.text).not.toMatch(/\d+\.\d+%/);
  });
  test("TTY curl failure retains its status and diagnostic", () => {
    const f = fixture();
    f.old();
    const result = f.run(["1.2.3"], { MODE: "failure" }, true);
    expect(result.status).not.toBe(0);
    expect(result.text).toContain("server failed");
    expect(readFileSync(join(f.prefix, "crabcode"), "utf8")).toContain("0.9.0");
  });
});
