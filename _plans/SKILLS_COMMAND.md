# /skills Command

## Overview

The `/skills` command lists all available skills discovered from the filesystem. Skills provide specialized domain-specific instructions and workflows that can be loaded by the LLM using the `skill` tool or invoked directly as slash commands.

## Skill Discovery Paths

Skills are discovered from the following locations (in order, matching OpenCode behavior):

### Global paths (`~/.config/`)

- `~/.config/opencode/skills/*/SKILL.md`
- `~/.config/opencode/skill/*/SKILL.md`
- `~/.config/crabcode/skills/*/SKILL.md`
- `~/.config/crabcode/skill/*/SKILL.md`
- `~/.claude/skills/*/SKILL.md` (Claude Code compat)
- `~/.agents/skills/*/SKILL.md` (Claude Code compat)

### Project paths (walking up from project root)

- `.opencode/skills/*/SKILL.md`
- `.opencode/skill/*/SKILL.md`
- `.crabcode/skills/*/SKILL.md`
- `.crabcode/skill/*/SKILL.md`
- `.claude/skills/*/SKILL.md` (at each ancestor directory)
- `.agents/skills/*/SKILL.md` (at each ancestor directory)

### Config paths (future)

- `config.skills.paths` — additional local directories to scan
- `config.skills.urls` — remote `.well-known/skills/` endpoints

## SKILL.md Format

Each skill is defined by a `SKILL.md` file with YAML frontmatter:

```markdown
---
name: my-skill
description: Description of what this skill does and when to use it.
---

# Skill content (markdown body)

Instructions, workflows, code examples, etc.
```

### Required fields

- `name` (string) — Unique skill identifier, used as the command name and tool parameter
- `description` (string) — Human-readable description shown in `/skills` dialog and system prompt

### Fallback parsing

If standard YAML parsing fails (e.g., values containing unquoted colons from Claude Code compat), a fallback sanitizer converts problematic values to YAML block scalars before retrying.

## Implementation Details

### Module: `src/skill/mod.rs`

- `SkillStore` — lazily initialized static store that holds all loaded skills
- `SkillLoader::load()` — scans all discovery paths for `SKILL.md` files
- `parse_skill_file()` — parses YAML frontmatter with fallback sanitization
- `fallback_sanitize_yaml()` — handles malformed YAML (Claude Code compat)

### Tool: `src/tools/skill.rs`

The `skill` tool is registered alongside other tools and can be invoked by the LLM:

- **Tool ID**: `skill`
- **Parameter**: `name` (string) — the skill name
- **Behavior**: Loads the skill's `SKILL.md` content and returns it wrapped in `<skill_content>` XML with base directory info and a sampled file list
- **Description**: Dynamically includes all available skills in XML format, matching the reference behavior

### System Prompt

Available skills are injected into the system prompt via `SystemPromptComposer::get_custom_instructions()` in `src/prompt/mod.rs`. They appear in `<available_skills>` XML block with name, description, and location.

### Dialog: `/skills`

The `/skills` command opens a dialog that lists all loaded skills with their name and description (from YAML frontmatter). Selecting a skill from the dialog does not currently auto-invoke it; that is handled by the LLM invoking the `skill` tool.

### Slash Commands

Each skill is automatically registered as a slash command (e.g., `/my-skill`). Typing the command injects the skill's markdown content into the conversation.

## Reference

Implementation mirrors the OpenCode skills system (`_dev_reference1/packages/opencode/src/skill/`):

- Same discovery paths and precedences
- Same YAML frontmatter parsing with fallback sanitization
- Same tool behavior with `<skill_content>` XML output
- Same `<available_skills>` format in system prompt
- Same skill-as-command registration
