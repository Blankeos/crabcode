const { test } = require("node:test");
const assert = require("node:assert/strict");
const { getPlatformInfo, downloadArchive } = require("./install");

const info = {
  extension: ".tar.gz",
  filename: "crabcode-test.tar.gz",
  url: "https://example.com/crabcode-test.tar.gz",
};

test("current platform uses registry-compatible archives", () => {
  assert.equal(
    getPlatformInfo().extension,
    process.platform === "win32" ? ".zip" : ".tar.gz",
  );
});

test("gzip download succeeds without fallback", async () => {
  const urls = [];
  assert.equal(
    await downloadArchive(info, "/unused", async (url) => urls.push(url)),
    info,
  );
  assert.deepEqual(urls, [info.url]);
});

test("older xz assets are retried only after gzip 404", async () => {
  const urls = [];
  const result = await downloadArchive(info, "/unused", async (url) => {
    urls.push(url);
    if (urls.length === 1)
      throw Object.assign(new Error("missing"), { statusCode: 404 });
  });
  assert.equal(result.extension, ".tar.xz");
  assert.deepEqual(urls, [
    info.url,
    "https://example.com/crabcode-test.tar.xz",
  ]);
});

test("network failures and Windows zip failures do not fallback", async () => {
  for (const [asset, error] of [
    [info, new Error("network")],
    [
      { ...info, extension: ".zip" },
      Object.assign(new Error("missing"), { statusCode: 404 }),
    ],
  ]) {
    let attempts = 0;
    await assert.rejects(
      downloadArchive(asset, "/unused", async () => {
        attempts++;
        throw error;
      }),
      error,
    );
    assert.equal(attempts, 1);
  }
});

const { Readable, Writable } = require("node:stream");
const {
  DownloadProgress,
  transferDownload,
  downloadFile,
  installedVersion,
} = require("./install");
function capture({ tty = true, columns = 80, env = { TERM: "xterm" } } = {}) {
  let text = "";
  const stderr = {
    isTTY: tty,
    columns,
    write(value) {
      text += value;
    },
  };
  return { progress: new DownloadProgress(stderr, env), text: () => text };
}
function response(chunks, total) {
  const stream = Readable.from(chunks);
  stream.headers =
    total === undefined ? {} : { "content-length": String(total) };
  return stream;
}
function sink() {
  return new Writable({
    write(chunk, encoding, done) {
      done();
    },
  });
}
const strip = (text) => text.replace(/\x1b\[[0-9;]*m/g, "");

test("known progress renders 0 and 17 percent, withholds 100 until successful finish", () => {
  const ui = capture();
  ui.progress.start(100);
  assert.match(ui.text(), /  0%/);
  ui.progress.advance(17);
  assert.match(ui.text(), / 17%/);
  assert.match(ui.text(), /\x1b\[33m█/);
  ui.progress.advance(83);
  assert.doesNotMatch(ui.text(), /100%/);
  ui.progress.finish(true);
  assert.match(ui.text(), /100%\n$/);
  assert.equal(
    strip(ui.text()).split("\r").pop().trim().split(" ")[0].length,
    50,
  );
});

test("unknown, invalid and zero totals remain static downloading text", async () => {
  for (const total of [undefined, 0, -1, "bad", Infinity]) {
    const ui = capture();
    await transferDownload(
      response([Buffer.from("abc")], total),
      sink(),
      ui.progress,
    );
    assert.equal(ui.text(), "Downloading...\n");
  }
});

test("bar fits narrow terminals and is capped at 50 cells", () => {
  for (const columns of [1, 2, 4, 5, 6, 7, 12, 20, 55, 80, 200]) {
    const ui = capture({ columns });
    ui.progress.start(100);
    ui.progress.advance(17);
    ui.progress.finish(true);
    for (const line of strip(ui.text()).split("\r").filter(Boolean)) {
      const visible = line.replace(/\n$/, "");
      assert.ok(visible.length < columns, `${columns}: ${visible}`);
      assert.ok((visible.match(/[█.]/g) || []).length <= 50);
    }
  }
});

test("redirected stderr and dumb terminals have no ANSI or carriage returns", async () => {
  for (const options of [{ tty: false }, { env: { TERM: "dumb" } }]) {
    const ui = capture(options);
    await transferDownload(
      response([Buffer.alloc(17), Buffer.alloc(83)], 100),
      sink(),
      ui.progress,
    );
    assert.equal(ui.text(), "Downloading...\n");
    assert.doesNotMatch(ui.text(), /[\x1b\r]/);
  }
});

test("NO_COLOR disables amber but retains interactive redraw", () => {
  const ui = capture({ env: { TERM: "xterm", NO_COLOR: "" } });
  ui.progress.start(100);
  ui.progress.advance(17);
  ui.progress.finish(true);
  assert.doesNotMatch(ui.text(), /\x1b/);
  assert.match(ui.text(), /\r/);
  assert.match(ui.text(), /100%\n$/);
});

test("successful pipeline waits for the destination before displaying 100", async () => {
  const ui = capture();
  let release;
  const file = new Writable({
    write(chunk, encoding, done) {
      done();
    },
    final(done) {
      release = done;
    },
  });
  const pending = transferDownload(
    response([Buffer.alloc(100)], 100),
    file,
    ui.progress,
  );
  while (!release) await new Promise((resolve) => setImmediate(resolve));
  assert.doesNotMatch(ui.text(), /100%/);
  release();
  await pending;
  assert.match(ui.text(), /100%\n$/);
});

test("truncation, abort, readable and writable errors close progress and clean partial files", async () => {
  for (const mode of [
    "truncated",
    "abort",
    "read-error",
    "write-error",
    "incomplete",
  ]) {
    const ui = capture();
    let stream = response([Buffer.alloc(17)], 100);
    if (mode === "abort" || mode === "read-error") {
      stream = new Readable({
        read() {
          this.push(Buffer.alloc(17));
          if (mode === "abort") this.destroy();
          else this.destroy(new Error("read failed"));
        },
      });
      stream.headers = { "content-length": "100" };
    }
    if (mode === "incomplete") {
      stream = response([Buffer.alloc(100)], 100);
      stream.complete = false;
    }
    const file =
      mode === "write-error"
        ? new Writable({
            write(chunk, encoding, done) {
              done(new Error("disk failed"));
            },
          })
        : sink();
    let removed = false;
    await assert.rejects(
      downloadFile("unused", "/partial", {
        fetch: async () => stream,
        createFile: () => file,
        progress: ui.progress,
        remove: (dest) => {
          assert.equal(dest, "/partial");
          removed = true;
        },
      }),
    );
    assert.equal(removed, true, mode);
    assert.equal(file.destroyed, true, mode);
    assert.match(ui.text(), /\n$/);
    assert.doesNotMatch(ui.text(), /100%/);
    const before = ui.text();
    ui.progress.finish(false);
    assert.equal(ui.text(), before);
  }
});

test("download success preserves archive; fetch errors do not remove an existing file", async () => {
  for (const fail of [false, true]) {
    const ui = capture();
    let removed = false;
    const pending = downloadFile("unused", "/archive", {
      fetch: async () => {
        if (fail) throw new Error("offline");
        return response([Buffer.alloc(100)], 100);
      },
      createFile: () => sink(),
      progress: ui.progress,
      remove: () => {
        removed = true;
      },
    });
    if (fail) await assert.rejects(pending, /offline/);
    else await pending;
    assert.equal(removed, false);
  }
});

test("installed version comes from bounded binary execution, not package metadata", () => {
  assert.equal(
    installedVersion(
      "/binary",
      () => {
        throw new Error("must not execute");
      },
      () => false,
    ),
    "not installed",
  );
  assert.equal(
    installedVersion(
      "/binary",
      (binary, args, options) => {
        assert.equal(binary, "/binary");
        assert.deepEqual(args, ["--version"]);
        assert.equal(options.timeout, 2000);
        assert.equal(options.maxBuffer, 4096);
        assert.deepEqual(options.stdio, ["ignore", "pipe", "ignore"]);
        return "crabcode 0.0.9\n";
      },
      () => true,
    ),
    "0.0.9",
  );
  assert.equal(
    installedVersion(
      "/binary",
      () => "crabcode v1.2.3-beta.1\n",
      () => true,
    ),
    "1.2.3-beta.1",
  );
  assert.equal(
    installedVersion(
      "/binary",
      () => {
        throw new Error("timeout");
      },
      () => true,
    ),
    "unknown",
  );
  assert.equal(
    installedVersion(
      "/binary",
      () => "\x1b[31mgarbage",
      () => true,
    ),
    "unknown",
  );
});
