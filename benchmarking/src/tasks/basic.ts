import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { DEFAULT_MODEL } from '../defaults.ts'
import { defineTask } from './define.ts'

export const basicTasks = [
  defineTask({
    id: 'bugfix-js',
    title: 'Fix a small JavaScript bug',
    difficulty: 'smoke',
    tags: ['javascript', 'bugfix', 'small'],
    files: {
      'package.json': JSON.stringify({ type: 'module' }, null, 2) + '\n',
      'stats.js': `export function average(nums) {
  if (nums.length === 0) return 0
  return nums.reduce((sum, n) => sum + n, 0)
}
`,
    },
    prompt: `Fix stats.js. average([2, 4, 6]) should return 4, average([10]) should return 10, and average([]) should keep returning 0. Keep the change minimal.`,
    check: (cwd) => {
      const stats = readFileSync(join(cwd, 'stats.js'), 'utf8')
      const hasDivide = /\/\s*nums\.length/.test(stats)
      const stillHandlesEmpty = /length\s*={2,3}\s*0/.test(stats) && /return\s+0/.test(stats)
      return [
        { name: 'divides by length', pass: hasDivide },
        { name: 'keeps empty-array guard', pass: stillHandlesEmpty },
      ]
    },
  }),
  defineTask({
    id: 'config-doc-sync',
    title: 'Synchronize tiny config docs',
    difficulty: 'smoke',
    tags: ['docs', 'json', 'sync'],
    files: {
      'config.json': JSON.stringify(
        {
          model: DEFAULT_MODEL,
          agent: { build: { steps: 20 } },
        },
        null,
        2,
      ) + '\n',
      'README.md': `# Fixture

Default model: openai/gpt-5.3-codex
Build steps: 12
`,
    },
    prompt: `Update README.md so the documented Build steps value matches config.json. Do not change config.json.`,
    check: (cwd) => {
      const config = readFileSync(join(cwd, 'config.json'), 'utf8')
      const readme = readFileSync(join(cwd, 'README.md'), 'utf8')
      return [
        { name: 'README documents 20 steps', pass: /Build steps:\s*20/.test(readme) },
        { name: 'config remains unchanged', pass: config.includes('"steps": 20') },
      ]
    },
  }),
]
