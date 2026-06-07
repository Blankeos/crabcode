import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { defineTask } from './define.ts'

export const rustTasks = [
  defineTask({
    id: 'add-rust-test',
    title: 'Add a focused Rust test',
    difficulty: 'smoke',
    tags: ['rust', 'tests', 'small'],
    files: {
      'src/lib.rs': `pub fn slugify(input: &str) -> String {
    input
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_basic_text() {
        assert_eq!(slugify("Hello Crab Code"), "hello-crab-code");
    }
}
`,
      'Cargo.toml': `[package]
name = "bench-fixture"
version = "0.0.0"
edition = "2021"
`,
    },
    prompt: `Add one focused test in src/lib.rs for slugify that covers leading/trailing whitespace and repeated internal whitespace. Do not change the slugify implementation.`,
    check: (cwd) => {
      const lib = readFileSync(join(cwd, 'src/lib.rs'), 'utf8')
      return [
        { name: 'adds a second test', pass: (lib.match(/#\[test\]/g) ?? []).length >= 2 },
        { name: 'covers whitespace case', pass: /\\t|\\n| {2,}|leading|trailing|whitespace/i.test(lib) },
        { name: 'does not change implementation shape', pass: lib.includes('.split_whitespace()') },
      ]
    },
  }),
]

