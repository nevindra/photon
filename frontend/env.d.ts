/// <reference types="vite/client" />

// Single-File Component shim: lets `.ts` code import `*.vue` files with a
// typed default export. Vue's tooling would infer richer types for
// `lang="ts"` SFCs, but this ambient fallback keeps imports of the many
// JS-authored `.vue` views from erroring under the type-check gate.
declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}
