import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { isAbsolute, resolve } from "node:path";

const PROTOCOL_VERSION = 1;
const protocolWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = process.stderr.write.bind(process.stderr);
console.log = console.error.bind(console);
console.info = console.error.bind(console);
console.debug = console.error.bind(console);
const plugins = [];
let workspace = process.cwd();

function reply(id, result) {
  protocolWrite(`${JSON.stringify({ id, result })}\n`);
}

function fail(id, error) {
  protocolWrite(
    `${JSON.stringify({
      id,
      error: {
        code: -32000,
        message: error instanceof Error ? error.message : String(error),
        data: error instanceof Error ? { stack: error.stack } : null,
      },
    })}\n`,
  );
}

function importTarget(source) {
  if (source.startsWith("file:")) {
    return source;
  }
  if (!source.startsWith(".") && !isAbsolute(source)) {
    return pathToFileURL(Bun.resolveSync(source, workspace)).href;
  }
  return pathToFileURL(resolve(workspace, source)).href;
}

async function loadPlugin(spec) {
  const module = await import(importTarget(spec.source));
  const factory = module.default ?? module.plugin ?? module;
  const hooks = typeof factory === "function"
    ? await factory({
        directory: workspace,
        worktree: workspace,
        options: spec.options,
        client: {
          app: {
            log(entry) {
              process.stderr.write(`[plugin:${spec.source}] ${JSON.stringify(entry)}\n`);
            },
          },
        },
      })
    : factory;
  plugins.push({ source: spec.source, hooks: hooks ?? {} });
  return { source: spec.source };
}

async function dispatch(method, params) {
  switch (method) {
    case "initialize": {
      if (params?.protocolVersion !== PROTOCOL_VERSION) {
        throw new Error(
          `unsupported protocol version ${params?.protocolVersion}; expected ${PROTOCOL_VERSION}`,
        );
      }
      workspace = params.workspace ?? workspace;
      return { protocolVersion: PROTOCOL_VERSION, runtime: `bun ${Bun.version}` };
    }
    case "load_plugins": {
      const loaded = [];
      for (const spec of params?.plugins ?? []) loaded.push(await loadPlugin(spec));
      return { loaded };
    }
    case "invoke_hook": {
      const output = params?.output ?? {};
      for (const plugin of plugins) {
        const hook = plugin.hooks?.[params?.hook];
        if (typeof hook === "function") await hook(params?.input ?? {}, output);
      }
      return output;
    }
    case "ping":
      return { ok: true };
    case "shutdown":
      setTimeout(() => process.exit(0), 0);
      return { ok: true };
    default:
      throw new Error(`unknown plugin host method: ${method}`);
  }
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of input) {
  if (!line.trim()) continue;
  let request;
  try {
    request = JSON.parse(line);
    reply(request.id, await dispatch(request.method, request.params));
  } catch (error) {
    fail(request?.id ?? null, error);
  }
}
