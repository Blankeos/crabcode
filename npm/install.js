#!/usr/bin/env node

const { execSync, execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");
const { pipeline, Transform } = require("stream");
const { promisify } = require("util");
const pipe = promisify(pipeline);

// Version should match your Rust crate version.
const VERSION = require("./package.json").version;
const BINARY_NAME = "crabcode";

class DownloadProgress {
  constructor(stderr = process.stderr, env = process.env) {
    this.stderr = stderr;
    this.redraw = Boolean(stderr.isTTY && env.TERM !== "dumb");
    this.color =
      this.redraw && !Object.prototype.hasOwnProperty.call(env, "NO_COLOR");
    this.total = null;
    this.bytes = 0;
    this.closed = false;
  }

  start(total) {
    const length = Number(total);
    this.total = Number.isSafeInteger(length) && length > 0 ? length : null;
    if (!this.redraw || !this.total) this.stderr.write("Downloading...\n");
    else this.render(false);
  }

  advance(bytes) {
    this.bytes += bytes;
    if (!this.closed && this.redraw && this.total) this.render(false);
  }

  render(success) {
    // Receipt of the final chunk is not success: the destination must finish too.
    const percent = success
      ? 100
      : Math.min(99, Math.floor((this.bytes / this.total) * 100));
    const columns = Number.isFinite(this.stderr.columns)
      ? Math.max(1, Math.floor(this.stderr.columns))
      : 80;
    const available = Math.max(0, columns - 1); // Avoid terminal auto-wrap.
    const suffix = ` ${String(percent).padStart(3)}%`;
    const width = Math.min(50, Math.max(0, available - suffix.length));
    const filled = Math.floor((width * percent) / 100);
    const blocks = "█".repeat(filled);
    const bar =
      (this.color && blocks ? `\x1b[33m${blocks}\x1b[0m` : blocks) +
      ".".repeat(width - filled);
    const text = width
      ? bar + suffix
      : available >= 4
        ? `${percent}%`
        : ".".repeat(available);
    this.stderr.write(`\r${text}`);
  }

  finish(success) {
    if (this.closed) return;
    if (this.redraw && this.total) {
      if (success) this.render(true);
      this.stderr.write("\n");
    }
    this.closed = true;
  }
}

function installedVersion(
  binaryPath,
  run = execFileSync,
  exists = fs.existsSync,
) {
  if (!exists(binaryPath)) return "not installed";
  try {
    const output = run(binaryPath, ["--version"], {
      timeout: 2000,
      maxBuffer: 4096,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    const match = /^crabcode\s+v?(\S+)$/i.exec(output);
    // Only expose a compact version token, never arbitrary terminal control text.
    const version = match ? match[1] : output;
    return /^[0-9][0-9A-Za-z.+-]*$/.test(version) ? version : "unknown";
  } catch (_) {
    return "unknown";
  }
}

async function transferDownload(response, file, progress) {
  progress.start(response.headers && response.headers["content-length"]);
  const counter = new Transform({
    transform(chunk, encoding, callback) {
      progress.advance(chunk.length);
      callback(null, chunk);
    },
  });
  try {
    await pipe(response, counter, file);
    if (
      response.complete === false ||
      (progress.total && progress.bytes !== progress.total)
    ) {
      throw new Error("Incomplete download");
    }
    progress.finish(true);
  } catch (error) {
    progress.finish(false);
    throw error;
  }
}

function getPlatformInfo() {
  const platform = process.platform;
  const arch = process.arch;

  // Map Node.js platform/arch to Rust target triples.
  const platformMap = {
    darwin: {
      x64: "x86_64-apple-darwin",
      arm64: "aarch64-apple-darwin",
    },
    linux: {
      x64: "x86_64-unknown-linux-gnu",
      arm64: "aarch64-unknown-linux-gnu",
    },
    win32: {
      x64: "x86_64-pc-windows-msvc",
    },
  };

  if (!platformMap[platform]) {
    throw new Error(`Unsupported platform: ${platform}`);
  }

  if (!platformMap[platform][arch]) {
    throw new Error(`Unsupported architecture: ${arch} on ${platform}`);
  }

  const target = platformMap[platform][arch];
  const extension = platform === "win32" ? ".zip" : ".tar.gz";
  const binaryName = platform === "win32" ? `${BINARY_NAME}.exe` : BINARY_NAME;

  return {
    target,
    extension,
    binaryName,
    filename: `${BINARY_NAME}-${target}${extension}`,
    url: `https://github.com/Blankeos/crabcode/releases/download/v${VERSION}/${BINARY_NAME}-${target}${extension}`,
  };
}

function fetchResponse(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) return reject(new Error("Too many download redirects"));
    https
      .get(url, (res) => {
        if ([301, 302, 303, 307, 308].includes(res.statusCode)) {
          res.resume();
          if (!res.headers.location)
            return reject(new Error("Missing redirect location"));
          resolve(
            fetchResponse(
              new URL(res.headers.location, url).href,
              redirects + 1,
            ),
          );
        } else if (res.statusCode === 200) {
          resolve(res);
        } else {
          res.resume();
          const error = new Error(
            `Failed to download: ${res.statusCode} ${res.statusMessage}`,
          );
          error.statusCode = res.statusCode;
          reject(error);
        }
      })
      .on("error", reject);
  });
}

async function downloadArchive(info, dest, download = downloadFile) {
  try {
    await download(info.url, dest);
    return info;
  } catch (error) {
    // Older releases published xz only. Do not retry transient/network errors.
    if (info.extension !== ".tar.gz" || error.statusCode !== 404) throw error;
    const legacy = {
      ...info,
      extension: ".tar.xz",
      filename: info.filename.replace(/\.tar\.gz$/, ".tar.xz"),
      url: info.url.replace(/\.tar\.gz$/, ".tar.xz"),
    };
    await download(legacy.url, dest);
    return legacy;
  }
}

async function downloadFile(
  url,
  dest,
  {
    fetch = fetchResponse,
    createFile = fs.createWriteStream,
    progress = new DownloadProgress(),
    remove = fs.unlinkSync,
  } = {},
) {
  let created = false;
  try {
    const response = await fetch(url);
    let file;
    try {
      file = createFile(dest);
      created = true;
    } catch (error) {
      response.destroy();
      throw error;
    }
    await transferDownload(response, file, progress);
  } catch (error) {
    progress.finish(false);
    if (created) {
      try {
        remove(dest);
      } catch (_) {
        /* Keep the original download error. */
      }
    }
    throw error;
  }
}

function extractArchive(archivePath, extractDir, platformInfo) {
  console.error("Extracting binary...");

  const cmd =
    platformInfo.extension === ".zip"
      ? `unzip -o "${archivePath}" -d "${extractDir}" 2>/dev/null || powershell -command "Expand-Archive -Path '${archivePath}' -DestinationPath '${extractDir}' -Force"`
      : `tar -xf "${archivePath}" -C "${extractDir}"`;

  // Extraction diagnostics must never corrupt the ACP stdout transport.
  execSync(cmd, { stdio: ["ignore", 2, 2] });
}

function logInstallFailure(error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error("Installation failed:", message);
  console.error("\nYou can install crabcode directly using:");
  console.error(
    'curl --proto "=https" --tlsv1.2 -LsSf https://github.com/Blankeos/crabcode/releases/latest/download/crabcode-installer.sh | sh',
  );
}

async function install({ exitOnComplete = false } = {}) {
  try {
    let platformInfo = getPlatformInfo();
    const binDir = path.join(__dirname, "bin");
    const archivePath = path.join(__dirname, platformInfo.filename);
    const binaryPath = path.join(binDir, platformInfo.binaryName);

    console.error(`Installed version: ${installedVersion(binaryPath)}`);
    console.error(`\nInstalling crabcode version: ${VERSION}`);

    if (!fs.existsSync(binDir)) fs.mkdirSync(binDir, { recursive: true });

    platformInfo = await downloadArchive(platformInfo, archivePath);
    extractArchive(archivePath, __dirname, platformInfo);
    console.error("Installing binary...");

    const extractedBinaryPath = path.join(__dirname, platformInfo.binaryName);
    if (fs.existsSync(extractedBinaryPath)) {
      fs.renameSync(extractedBinaryPath, binaryPath);
    } else {
      const subdirPath = path.join(
        __dirname,
        `${BINARY_NAME}-${platformInfo.target}`,
        platformInfo.binaryName,
      );
      if (fs.existsSync(subdirPath)) {
        fs.renameSync(subdirPath, binaryPath);
        fs.rmSync(path.dirname(subdirPath), { recursive: true, force: true });
      } else {
        throw new Error("Binary not found after extraction");
      }
    }

    if (process.platform !== "win32") {
      fs.chmodSync(binaryPath, 0o755);
    }

    fs.unlinkSync(archivePath);
    console.error(`crabcode v${VERSION} installed successfully!`);

    if (exitOnComplete) {
      process.exit(0);
      return binaryPath;
    }

    return binaryPath;
  } catch (error) {
    logInstallFailure(error);

    if (exitOnComplete) {
      process.exit(1);
      return;
    }

    throw error;
  }
}

// Only run install if this script is executed directly.
if (require.main === module) {
  install({ exitOnComplete: true });
}

module.exports = {
  getPlatformInfo,
  downloadArchive,
  downloadFile,
  transferDownload,
  DownloadProgress,
  installedVersion,
  install,
};
