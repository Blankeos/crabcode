#!/usr/bin/env bash

# This script bumps crate/npm versions, commits the release and creates a git tag.
# - Keep the tree clean before running.
# - Optionally pass patch|minor|major as first arg or answer the prompt.

set -euo pipefail

if [ -n "$(git status --porcelain)" ]; then
  echo "Please commit all changes before running a release bump." >&2
  exit 1
fi

NAME=$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' Cargo.toml)
CURRENT=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml)

if [ $# -gt 1 ]; then
  echo "Usage: ./scripts/tag_and_release.sh [patch|minor|major]" >&2
  exit 1
fi

BUMP="${1-}"

if [ -z "$BUMP" ]; then
  echo "What kind of release bump for $NAME? (current version: $CURRENT) [patch, minor, major]"
  read -r BUMP
fi

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

if [ -z "$MAJOR" ] || [ -z "$MINOR" ] || [ -z "$PATCH" ]; then
  echo "Failed to parse current version: $CURRENT" >&2
  exit 1
fi

case "$BUMP" in
  patch) NEW="$MAJOR.$MINOR.$((PATCH + 1))" ;;
  minor) NEW="$MAJOR.$((MINOR + 1)).0" ;;
  major) NEW="$((MAJOR + 1)).0.0" ;;
  *) echo "Please specify patch, minor, or major" >&2; exit 1 ;;
esac

echo "Will bump ${CURRENT} -> ${NEW} and create git tag v${NEW}"
read -p "Proceed? [Y/n] " -r CONFIRM
CONFIRM=${CONFIRM:-Y}
if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
  echo "Aborted."
  exit 0
fi

echo "Updating Cargo.toml version to ${NEW}"
sed -i.bak "s/^version *= *\"[^\"]*\"/version = \"${NEW}\"/" Cargo.toml
rm -f Cargo.toml.bak

if [ -f "npm/package.json" ]; then
  echo "Updating npm/package.json version to ${NEW}"
  sed -i.bak "s/\"version\":[[:space:]]*\"[^\"]*\"/\"version\": \"${NEW}\"/" npm/package.json
  rm -f npm/package.json.bak
  git add npm/package.json
fi

git add Cargo.toml
git commit -m "release: ${NAME} v${NEW}"

echo "Creating git tag v${NEW}"
git tag "v${NEW}"

echo "Pushing commit and tag"
git push
git push --tags

echo "Done: ${NAME} v${NEW}"
