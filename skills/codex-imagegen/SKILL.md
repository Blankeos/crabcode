---
name: "codex-imagegen"
description: "Generate or edit raster images by delegating to the user-installed official Codex CLI with `codex exec`. Use when the task benefits from AI-created bitmap visuals such as photos, illustrations, textures, sprites, mockups, product mockups, wireframes, transparent-style cutouts, or repo assets, and the output should be a PNG/JPEG/WebP file rather than code, SVG, CSS, or canvas. Requires Codex CLI to be installed and authenticated with `codex login`."
---

# Codex Image Generation Skill

Generates or edits raster images for the current project by delegating to the official OpenAI Codex CLI via `codex exec`.

This skill is intentionally a subprocess bridge. Crabcode must not access Codex credentials directly, copy Codex auth headers, read `~/.codex/auth.json`, or call private Codex/ChatGPT backend endpoints itself.

## Top-level mode

This skill has exactly one mode:

- **Codex CLI delegation mode:** call the user-installed `codex` binary with `codex exec` and instruct Codex to use its own `imagegen` skill/tool. Codex handles authentication, model/tool access, image generation, and any subscription/quota accounting.

There is no API-key fallback in this skill.

## Authentication rule

Before attempting generation, assume the user must have already run:

```sh
codex login
```

If `codex exec` fails because Codex is missing, unauthenticated, expired, or otherwise unable to access its account, respond exactly:

```text
You must authenticate w/ `codex login`
```

Do not suggest `OPENAI_API_KEY` for this skill. Do not try to inspect or repair Codex credentials.

## When to use

Use this skill when the user asks Crabcode to create, edit, transform, or derive bitmap assets, including:

- website or app hero images
- product mockups
- UI mockups or wireframes as images
- photos or photorealistic renders
- illustrations
- textures
- game sprites
- thumbnails
- icons that should be raster assets
- transparent-background or cutout-style PNGs
- variants based on an existing reference image

Do not use this skill when the task is better solved by:

- editing existing SVG/vector assets
- creating repo-native HTML/CSS/canvas
- extending an established logo/icon system in vector form
- making textual diagrams or Mermaid diagrams
- generating code instead of a bitmap file

## Safety and boundary rules

- Always use the installed `codex` CLI as the actor that talks to OpenAI.
- Never read `~/.codex/auth.json` or any Codex token store.
- Never spoof Codex headers, user agents, account IDs, or private endpoints.
- Never call `https://chatgpt.com/backend-api/codex` directly from Crabcode.
- Never use `--dangerously-bypass-approvals-and-sandbox` by default.
- Prefer `--skip-git-repo-check` because image output may be requested from temporary or asset folders.
- Keep the delegated prompt tightly scoped to image generation/editing and saving the output file.
- Instruct Codex not to modify unrelated files.
- After `codex exec` returns, verify the expected output file exists before reporting success.

## Output path policy

Always establish a concrete destination path before invoking `codex exec`.

Path precedence:

1. If the user names a destination file, use that path.
2. If the image is intended for the current project but no path is specified, choose an appropriate path under the workspace, such as:
   - `assets/<descriptive-name>.png`
   - `public/images/<descriptive-name>.png`
   - `src/assets/<descriptive-name>.png`
   - `output/imagegen/<descriptive-name>.png`
3. If the image is only for brainstorming or preview, use `output/imagegen/<descriptive-name>.png` in the current workspace.

Prefer `.png` unless the user explicitly requests JPEG or WebP.

Create parent directories locally before calling Codex when practical.

## Basic command shape

Use this pattern:

```sh
mkdir -p "$(dirname "$OUTPUT_PATH")"
codex exec --skip-git-repo-check \
  "Use your imagegen skill to generate: $IMAGE_REQUEST

Save or copy the final image exactly to: $ABSOLUTE_OUTPUT_PATH
Do not modify anything else.
If you are not authenticated, respond exactly: You must authenticate w/ \`codex login\`.
When done, reply with only the absolute saved path."
```

After the command finishes:

```sh
test -f "$OUTPUT_PATH"
```

If the file exists, report the saved path to the user. If it does not exist, inspect Codex stdout/stderr enough to determine whether this is an auth failure, CLI failure, or generation failure.

## Prompting Codex

The delegated prompt should include:

- the exact user image request
- style, composition, aspect ratio, size, background, and format constraints from the user
- any reference image paths, if provided
- the exact absolute output path
- an instruction to not modify unrelated files
- the exact auth failure response
- an instruction to reply only with the saved path

Example delegated prompt:

```text
Use your imagegen skill to generate a square PNG illustration of a cozy crab-shaped coding robot at a terminal, warm desk lamp, dark background, polished but playful.

Save or copy the final image exactly to: /absolute/path/to/public/images/crab-robot.png
Do not modify anything else.
If you are not authenticated, respond exactly: You must authenticate w/ `codex login`.
When done, reply with only the absolute saved path.
```

## Existing image editing

If the user provides one or more reference images, pass their absolute paths in the delegated prompt and clearly describe how Codex should use them.

Example:

```text
Use your imagegen skill to edit the reference image at /absolute/path/to/input.png.
Change the background to a clean studio gradient, preserve the object shape and colors, and export a PNG.

Save or copy the final image exactly to: /absolute/path/to/output/product-studio.png
Do not modify anything else.
If you are not authenticated, respond exactly: You must authenticate w/ `codex login`.
When done, reply with only the absolute saved path.
```

Do not embed large image files in the shell command. Prefer passing local file paths.

## Transparent-background requests

For transparent-background or cutout requests, stay in Codex CLI delegation mode. Ask Codex to use its own imagegen workflow and save the final PNG to the requested path.

Example phrasing:

```text
Use your imagegen skill to create a PNG cutout with a transparent background if your imagegen workflow supports it. If not, use the best supported workflow to produce a clean removable-background PNG.
```

Do not implement local chroma-key removal in this skill unless the user explicitly asks for local post-processing after generation.

## Batch generation

For multiple assets, prefer one `codex exec` call per final asset unless the user explicitly asks for a batch in one call.

Each requested image should have a distinct output path. Verify every expected file exists.

Example paths:

```text
output/imagegen/icon-idle.png
output/imagegen/icon-hover.png
output/imagegen/icon-active.png
```

## Failure handling

If `codex` is not installed, unavailable on `PATH`, unauthenticated, or returns an auth/account error, respond exactly:

```text
You must authenticate w/ `codex login`
```

If Codex succeeds but the expected file is missing:

- report that Codex did not create the expected output file
- include the expected path
- do not claim success
- optionally suggest rerunning with a simpler prompt or explicit output filename

If Codex creates a file at a different path and reports it clearly, move or copy it to the requested output path only if that is safe and unambiguous. Then verify the requested path exists.

## Completion response

On success, keep the final response short:

```text
Generated image saved to `<path>`.
```

If multiple files were generated:

```text
Generated images:
- `<path-1>`
- `<path-2>`
```

Do not include Codex internals, credentials, account details, or raw base64 image data.
