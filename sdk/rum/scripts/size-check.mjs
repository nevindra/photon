import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
const LIMIT = 5 * 1024;
const bytes = gzipSync(readFileSync(new URL("../dist/photon-rum.js", import.meta.url))).length;
console.log(`@photon/rum core: ${bytes} B gzipped (limit ${LIMIT})`);
if (bytes > LIMIT) {
  console.error(`FAIL: bundle ${bytes} B exceeds ${LIMIT} B`);
  process.exit(1);
}
