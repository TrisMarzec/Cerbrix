// What the tests that read source files (source.test.ts, wrapping.test.ts, HostPage.test.tsx) use
// of Node: two functions of node:fs and Vitest's `import.meta.dirname`. They are declared here
// instead of taking @types/node into the program, whose globals would type `setTimeout` in the
// browser code as Node's. (A `/// <reference types="node" />` in a test file would not help: it
// pulls the globals into the whole program.) Vite's `?raw` import is no way out either: under
// Vitest a CSS file reads as an empty string. Browser code must not use any of this.
declare module 'node:fs' {
  interface DirectoryEntry {
    name: string;
    isDirectory(): boolean;
  }

  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function readdirSync(path: string, options: { withFileTypes: true }): DirectoryEntry[];
}

interface ImportMeta {
  /** The directory of the running test file; set by Vitest, undefined in the browser. */
  readonly dirname: string;
}
