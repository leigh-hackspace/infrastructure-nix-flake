import { cpSync, createReadStream, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';
import solid from 'vite-plugin-solid';
import { defineConfig } from 'vite';

// Fixed, unhashed output names (assets/index.js, assets/index.css): the Rust
// binary embeds the dist tree via generated include_bytes! paths
// (gen_assets.sh), so the layout must be stable across builds.

// The vendored OpenWakeWord engine + onnxruntime-web + ONNX models are served
// as raw ES modules / WASM (see ../openwakeword/README.md) — they must NOT go
// through the bundler. OWW_DIR lets the Nix build point at a store path: the
// assets live outside web/app, so they don't survive the Nix source copy.
const owwDir = process.env.OWW_DIR ?? fileURLToPath(new URL('../openwakeword', import.meta.url));
const OWW_ITEMS = ['engine', 'ort', 'models', 'README.md'];

function copyOpenWakeWord(): Plugin {
  let outDir = 'dist';
  return {
    name: 'copy-openwakeword',
    apply: 'build',
    configResolved(config) {
      outDir = config.build.outDir;
    },
    closeBundle() {
      for (const item of OWW_ITEMS) {
        const src = path.join(owwDir, item);
        if (existsSync(src)) cpSync(src, path.join(outDir, item), { recursive: true });
      }
    },
    // Serve the vendored assets at /engine, /ort, /models in `vite dev`.
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const m = req.url?.match(/^\/(engine|ort|models)\/(.+)$/);
        if (!m) return next();
        const base = path.join(owwDir, m[1]);
        const file = path.resolve(base, m[2]);
        if (!file.startsWith(base + path.sep) || !existsSync(file)) return next();
        const type = file.endsWith('.wasm')
          ? 'application/wasm'
          : file.endsWith('.onnx')
            ? 'application/octet-stream'
            : 'text/javascript; charset=utf-8';
        res.setHeader('Content-Type', type);
        createReadStream(file).pipe(res);
      });
    },
  };
}

export default defineConfig({
  plugins: [solid(), copyOpenWakeWord()],
  build: {
    target: 'es2022',
    rollupOptions: {
      output: {
        entryFileNames: 'assets/index.js',
        assetFileNames: 'assets/[name][extname]',
      },
    },
  },
});
