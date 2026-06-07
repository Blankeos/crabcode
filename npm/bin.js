#!/usr/bin/env node

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");
const { install } = require("./install");

const binaryName = process.platform === "win32" ? "crabcode.exe" : "crabcode";
const binaryPath = path.join(__dirname, "bin", binaryName);

async function ensureBinary() {
  if (fs.existsSync(binaryPath)) {
    return;
  }

  console.error("crabcode binary not found. Attempting download...");

  try {
    await install();
  } catch (error) {
    process.exit(1);
  }

  if (!fs.existsSync(binaryPath)) {
    console.error("❌ crabcode binary still missing after download.");
    process.exit(1);
  }
}

async function run() {
  await ensureBinary();

  const child = spawn(binaryPath, process.argv.slice(2), { stdio: "inherit" });

  child.on("error", (err) => {
    console.error("❌ Failed to start crabcode:", err.message);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    process.exit(signal ? 1 : code || 0);
  });
}

run();
