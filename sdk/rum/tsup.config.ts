import { defineConfig } from "tsup";
export default defineConfig([
  {
    entry: { "photon-rum": "src/index.ts" },
    format: ["esm"],
    dts: { entry: { index: "src/index.ts" } },
    minify: true,
    treeshake: true,
    clean: true,
  },
  {
    entry: { "photon-rum.iife": "src/index.ts" },
    format: ["iife"],
    globalName: "PhotonRUM",
    minify: true,
    treeshake: true,
    outExtension: () => ({ js: ".js" }),
  },
]);
