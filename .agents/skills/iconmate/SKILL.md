---
name: iconmate
description: >-
  Use when the user asks to add, search, list, or manage SVG icons in a JS/TS project.
  Trigger phrases: "add an icon", "search for icons", "find an icon", "add svg",
  "icon for heart", "list icons", "delete icon", "rename icon", "sync icons",
  "reconcile icons", "iconmate", "iconify".
metadata:
  author: blankeos
  version: "1.2.0"
  repository: https://github.com/Blankeos/iconmate
license: MIT
---

# iconmate

Add SVG icons to JS/TS projects without icon libraries. Uses Iconify's 200k+ icons or any SVG source.

## Prerequisites

`iconmate` must be installed. If not available, install with any of these methods:

```bash
# npm / pnpm / bun (prebuilt binary via npm)
npm install -g iconmate
pnpm add -g iconmate
bun add -g iconmate

# Run without installing
npx iconmate
pnpm dlx iconmate
bunx iconmate

# Cargo (build from source)
cargo install iconmate

# Cargo binstall (prebuilt binary via cargo)
cargo binstall iconmate
```

## Workflow

Follow these steps when the user wants to add icons:

### 1. Search for icons

```bash
iconmate iconify search <query> --format json --limit 20 --include-collections
```

Present results to the user and let them pick.

### 2. Add the icon

```bash
iconmate add --folder <folder> --icon <prefix:name> --name <PascalCaseName>
```

- `--folder`: Icon output directory (default: `src/assets/icons`, or `assets/icons` when preset is `flutter`). Check `iconmate.config.json` or `iconmate.config.jsonc` in the project root for a configured folder.
- `--icon`: Accepts an Iconify name (e.g. `mdi:heart`), a URL, or raw SVG markup.
- `--name`: Optional when `--icon` is a URL or iconify id (iconmate auto-infers from the icon name). PascalCase for JS presets (e.g. `Heart`), lowerCamelCase for the `flutter` preset (e.g. `heart`). On collision, iconmate falls back to the collection-prefixed form (e.g. `MdiHeart` / `mdiHeart`).
- `--preset`: Framework preset. Check config for the project default.
  - `normal` → `.svg` (plain SVG file)
  - `react` → `.tsx` (React component)
  - `svelte` → `.svelte` (Svelte component)
  - `solid` → `.tsx` (SolidJS component)
  - `vue` → `.vue` (Vue component)
  - `emptysvg` → `.svg` (empty placeholder SVG)
  - `flutter` → `.svg` + regenerated Dart barrel at `lib/icons.dart`. Auto-selected when a `pubspec.yaml` with a `flutter:` section is detected. Use with `--flutter-barrel-file` (default `lib/icons.dart`) and `--flutter-barrel-class` (default `AppIcons`) to customize barrel output. iconmate owns `lib/icons.dart` entirely — call sites use `AppIcons.<name>` (e.g. `SvgPicture.asset(AppIcons.heart, width: 24)`).

### 3. Verify

```bash
iconmate list --folder <folder>
```

Confirm the icon appears in the list and the `index.ts` export file is updated.

## Other Commands

### Delete an icon

`iconmate delete` has two modes:

**Non-interactive (use this as an agent):** pass `--name` and/or `--filename` (repeatable) plus `-y` to confirm. `--name` matches the export identifier exactly as it appears in `index.ts` (e.g. `IconHeart`, including any prefix from the template). `--filename` matches the import path (e.g. `./heart.svg`). Run `iconmate list --folder <folder>` first to see the exact names and paths.

```bash
iconmate delete --folder <folder> --name IconHeart --name IconStar -y
iconmate delete --folder <folder> --filename ./heart.svg -y
```

Without `-y` the command refuses to proceed. Unknown names/filenames and ambiguous matches are hard errors (no partial deletes).

**Interactive (TUI picker):** with no `--name`/`--filename`, it opens a MultiSelect for the user. Do **not** run this form yourself — tell the user to run it:

```bash
iconmate delete --folder <folder>
```

### List current icons

```bash
iconmate list
```

### Sync (reconcile barrel with disk)

Use when an SVG was added or removed manually and the barrel (`index.ts` / `lib/icons.dart`) drifted out of sync.

```bash
# Dry-run — prints the plan, writes nothing. Always safe to run first.
iconmate sync --folder <folder>

# Apply additions (orphan SVGs → new barrel entries). Non-destructive.
iconmate sync --folder <folder> --apply

# Also remove orphan entries (barrel points at a missing SVG).
iconmate sync --folder <folder> --apply --prune

# Resolve a collision by overriding the inferred identifier.
iconmate sync --folder <folder> --apply --rename IconHeart=IconHeart2
```

Output is deterministic and parseable. Exits non-zero when collisions exist. `sync` only edits the barrel — it never touches SVG files.

### Browse Iconify collections

```bash
# List all collections
iconmate iconify collections

# List icons in a collection
iconmate iconify collection mdi

# Get raw SVG for one icon
iconmate iconify get mdi:heart
```

### Add from URL or raw SVG

```bash
# From URL
iconmate add --folder src/assets/icons --icon https://api.iconify.design/mdi:heart.svg --name Heart

# From raw SVG
iconmate add --folder src/assets/icons --icon '<svg>...</svg>' --name Heart
```

### Custom export template

```bash
iconmate add --folder src/assets/icons --icon mdi:heart --name Heart \
  --output-line-template "export { ReactComponent as Icon%name% } from './%icon%.svg?react';"
```

Template variables: `%name%` (PascalCase alias), `%icon%` (filename without extension), `%ext%` (file extension).

## Configuration

Check for `iconmate.config.json` or `iconmate.config.jsonc` in the project root. If present, respect its `folder`, `preset`, and `output_line_template` values. CLI flags override config values.

## Tips

- Always use `--format json` when searching so you can parse results programmatically.
- When adding multiple icons, run each `iconmate add` command separately. The export index is updated automatically after each add.
- For prototyping, use `--preset emptysvg` to create placeholder icons.
- The user imports icons like: `import { IconHeart } from "@/assets/icons";`
