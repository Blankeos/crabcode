import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { defineTask } from './define.ts'

export const siteTasks = [
  defineTask({
    id: 'local-site-fetch',
    title: 'Fetch local site data and update docs',
    difficulty: 'medium',
    tags: ['webfetch', 'docs', 'json'],
    site: {
      root: 'site',
    },
    files: {
      'site/api/releases.json': JSON.stringify(
        {
          releases: [
            {
              version: '1.8.0-beta.1',
              channel: 'beta',
              recommended: false,
              migrationNote: 'Beta users should keep the experimental flag enabled.',
            },
            {
              version: '1.7.4',
              channel: 'stable',
              recommended: true,
              migrationNote: 'Set `snapshotMode` to `sparse` before rollout.',
            },
          ],
        },
        null,
        2,
      ) + '\n',
      'docs/release.md': `# Release Notes

Recommended stable: 1.6.2
Migration note: TBD
`,
    },
    prompt: `Fetch {siteUrl}/api/releases.json, find the recommended stable release, and update docs/release.md with its version and migrationNote. Do not change files under site/.`,
    check: (cwd) => {
      const doc = readFileSync(join(cwd, 'docs/release.md'), 'utf8')
      const siteData = readFileSync(join(cwd, 'site/api/releases.json'), 'utf8')
      return [
        { name: 'documents recommended stable version', pass: /1\.7\.4/.test(doc) },
        { name: 'copies fetched migration note', pass: /snapshotMode/.test(doc) && /sparse/.test(doc) },
        { name: 'removes placeholder note', pass: !/TBD/.test(doc) },
        { name: 'keeps served fixture intact', pass: siteData.includes('"version": "1.7.4"') },
      ]
    },
  }),
]

