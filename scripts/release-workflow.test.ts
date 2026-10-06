import { afterEach, describe, expect, test } from "bun:test";
import { YAML } from "bun";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dir, "..");
const workflow = YAML.parse(
  readFileSync(join(root, ".github/workflows/publish-registries.yml"), "utf8"),
) as { jobs: Record<string, { steps: { name?: string; run?: string }[] }> };
const prepareScript = workflow.jobs.prepare.steps.find(
  (step) => step.name === "Verify release tag and package versions",
)!.run!;
const waitScript = workflow.jobs["wait-for-binaries"].steps.find(
  (step) => step.name === "Wait for GitHub release binaries",
)!.run!;
const config = readFileSync(join(root, "dist-workspace.toml"), "utf8");
const tempDirs: string[] = [];

function fixture(distConfig = config) {
  const dir = mkdtempSync(join(tmpdir(), "crabcode-release-test-"));
  tempDirs.push(dir);
  mkdirSync(join(dir, "npm"));
  mkdirSync(join(dir, "tools"));
  writeFileSync(
    join(dir, "Cargo.toml"),
    '[package]\nname = "crabcode"\nversion = "0.0.14"\n',
  );
  writeFileSync(join(dir, "npm/package.json"), '{"version":"0.0.14"}');
  writeFileSync(join(dir, "dist-workspace.toml"), distConfig);
  writeFileSync(join(dir, "output"), "");
  writeFileSync(join(dir, "assets"), "");

  function tool(name: string, script: string) {
    writeFileSync(
      join(dir, "tools", name),
      `#!/usr/bin/env bash\n${script}\n`,
      { mode: 0o755 },
    );
  }
  // Only the asset gate is under test; the ancestry check has already passed in CI.
  tool(
    "git",
    'if [[ "$1" == "rev-parse" ]]; then echo release-commit; fi\nexit 0',
  );
  tool(
    "gh",
    `printf '%s\\n' "$*" >> "$MOCK_GH_CALLS"
if [[ -n "\${MOCK_GH_ERROR:-}" ]]; then
  echo "$MOCK_GH_ERROR" >&2
  exit 1
fi
cat "$MOCK_ASSETS"`,
  );
  // Stop at the first wait rather than actually sleeping for an hour.
  tool("sleep", "exit 42");
  const env = {
    ...process.env,
    PATH: `${join(dir, "tools")}:${process.env.PATH}`,
    RELEASE_TAG: "v0.0.14",
    RELEASE_REPO: "Blankeos/crabcode",
    GITHUB_OUTPUT: join(dir, "output"),
    MOCK_ASSETS: join(dir, "assets"),
    MOCK_GH_CALLS: join(dir, "gh-calls"),
  };
  function run(script: string, overrides: Record<string, string> = {}) {
    return spawnSync("bash", ["-c", script], {
      cwd: dir,
      env: { ...env, ...overrides },
      encoding: "utf8",
    });
  }
  function prepare() {
    const result = run(prepareScript);
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    const outputs = Object.fromEntries(
      readFileSync(env.GITHUB_OUTPUT, "utf8")
        .trim()
        .split("\n")
        .map((line) => {
          const equals = line.indexOf("=");
          return [line.slice(0, equals), line.slice(equals + 1)];
        }),
    );
    expect(outputs.tag).toBe("v0.0.14");
    expect(outputs.version).toBe("0.0.14");
    return JSON.parse(outputs.assets) as string[];
  }
  function wait(
    expected: string[],
    actual: string[],
    overrides: Record<string, string> = {},
  ) {
    writeFileSync(env.MOCK_ASSETS, actual.join("\n"));
    return run(waitScript, {
      RELEASE_ASSETS: JSON.stringify(expected),
      ...overrides,
    });
  }
  return { dir, prepare, run, wait };
}

afterEach(() => {
  for (const dir of tempDirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

describe("registry release asset gate", () => {
  test("uses configured gzip/zip names and accepts a complete release", () => {
    const f = fixture();
    const assets = f.prepare();
    expect(assets).toEqual([
      "crabcode-aarch64-apple-darwin.tar.gz",
      "crabcode-aarch64-unknown-linux-gnu.tar.gz",
      "crabcode-x86_64-apple-darwin.tar.gz",
      "crabcode-x86_64-unknown-linux-gnu.tar.gz",
      "crabcode-x86_64-pc-windows-msvc.zip",
    ]);
    const result = f.wait(assets, [
      ...assets,
      "sha256.sum",
      "dist-manifest.json",
    ]);
    expect(result.status).toBe(0);
    expect(result.stdout).toContain(
      "All GitHub release binaries are available",
    );
    expect(readFileSync(join(f.dir, "gh-calls"), "utf8")).toContain(
      "release view v0.0.14 --repo Blankeos/crabcode",
    );
  });

  test.each([0, 1, 2, 3, 4])("waits when target %i is missing", (index) => {
    const f = fixture();
    const assets = f.prepare();
    const result = f.wait(
      assets,
      assets.filter((_, i) => i !== index),
    );
    expect(result.status).toBe(42);
    expect(result.stdout).toContain(assets[index]!);
    expect(result.stdout).not.toContain(
      "All GitHub release binaries are available",
    );
  });

  test("does not mistake checksum files for binary archives", () => {
    const f = fixture();
    const assets = f.prepare();
    const result = f.wait(
      assets,
      assets.map((name) => `${name}.sha256`),
    );
    expect(result.status).toBe(42);
    for (const name of assets) expect(result.stdout).toContain(name);
  });

  test("supports older tagged configs with cargo-dist's xz default", () => {
    const f = fixture(
      config
        .replace(/^unix-archive = .*\n/m, "")
        .replace(/^windows-archive = .*\n/m, ""),
    );
    const assets = f.prepare();
    expect(assets.filter((name) => name.endsWith(".tar.xz"))).toHaveLength(4);
    expect(assets.filter((name) => name.endsWith(".zip"))).toHaveLength(1);
    expect(f.wait(assets, assets).status).toBe(0);
  });

  test("automatically requires newly configured targets", () => {
    const f = fixture(
      config.replace("targets = [", 'targets = ["aarch64-pc-windows-msvc", '),
    );
    const assets = f.prepare();
    expect(assets).toContain("crabcode-aarch64-pc-windows-msvc.zip");
    expect(f.wait(assets, assets.slice(1)).status).toBe(42);
    expect(f.wait(assets, assets).status).toBe(0);
  });

  test("rejects an empty target configuration rather than silently publishing", () => {
    const f = fixture(config.replace(/^targets = .*$/m, "targets = []"));
    const result = f.run(prepareScript);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("No release targets configured");
  });

  test("shows GitHub API failures instead of hiding them as missing binaries", () => {
    const f = fixture();
    const assets = f.prepare();
    const result = f.wait(assets, [], {
      MOCK_GH_ERROR: "HTTP 403: API rate limit exceeded",
    });
    expect(result.status).toBe(42);
    expect(result.stderr).toContain("HTTP 403: API rate limit exceeded");
    expect(result.stdout).toContain("Unable to fetch GitHub release assets");
    expect(result.stdout).not.toContain(
      "All GitHub release binaries are available",
    );
  });
});
