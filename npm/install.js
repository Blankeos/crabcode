#!/usr/bin/env node

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");

// Version should match your Rust crate version.
const VERSION = require("./package.json").version;
const BINARY_NAME = "crabcode";

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
    https.get(url, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode)) {
        res.resume();
        if (!res.headers.location) return reject(new Error("Missing redirect location"));
        resolve(fetchResponse(new URL(res.headers.location, url).href, redirects + 1));
      } else if (res.statusCode === 200) {
        resolve(res);
      } else {
        res.resume();
        const error = new Error(`Failed to download: ${res.statusCode} ${res.statusMessage}`);
        error.statusCode = res.statusCode;
        reject(error);
      }
    }).on("error", reject);
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

async function downloadFile(url, dest) {
  console.error(`Downloading ${url}...`);

  const response = await fetchResponse(url);
  const file = fs.createWriteStream(dest);
  response.pipe(file);
  return new Promise((resolve, reject) => {
    file.on("finish", () => {
      file.close();
      resolve();
    });
    file.on("error", (err) => {
      fs.unlink(dest, () => {});
      reject(err);
    });
  });
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

    if (!fs.existsSync(binDir)) fs.mkdirSync(binDir, { recursive: true });

    platformInfo = await downloadArchive(platformInfo, archivePath);
    extractArchive(archivePath, __dirname, platformInfo);

    const extractedBinaryPath = path.join(__dirname, platformInfo.binaryName);
    if (fs.existsSync(extractedBinaryPath)) {
      fs.renameSync(extractedBinaryPath, binaryPath);
    } else {
      const subdirPath = path.join(__dirname, `${BINARY_NAME}-${platformInfo.target}`, platformInfo.binaryName);
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

module.exports = { getPlatformInfo, downloadArchive, install };
