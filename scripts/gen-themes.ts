// @ts-nocheck

import { writeFileSync, mkdirSync } from 'fs';
import { join } from 'path';

const GITHUB_API_URL = 'https://api.github.com/repos/anomalyco/opencode/contents/packages/ui/src/theme/themes';
const THEMES_DIR = join(process.cwd(), 'src', 'themes');

interface GitHubFile {
  name: string;
  download_url: string;
}

async function fetchThemes() {
  const response = await fetch(GITHUB_API_URL);
  if (!response.ok) {
    throw new Error(`Failed to fetch themes: ${response.statusText}`);
  }

  const files: GitHubFile[] = await response.json();

  mkdirSync(THEMES_DIR, { recursive: true });

  for (const file of files) {
    if (!file.name.endsWith('.json')) continue;

    console.log(`Fetching ${file.name}...`);
    const themeResponse = await fetch(file.download_url);
    if (!themeResponse.ok) {
      console.error(`Failed to fetch ${file.name}: ${themeResponse.statusText}`);
      continue;
    }

    const themeContent = await themeResponse.text();
    const themePath = join(THEMES_DIR, file.name);
    writeFileSync(themePath, themeContent);
    console.log(`Saved ${file.name}`);
  }

  console.log(`\nDone! Themes saved to ${THEMES_DIR}`);
}

fetchThemes().catch(console.error);
