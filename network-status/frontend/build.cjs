// Bundles the SolidJS + TypeScript frontend into static assets for the
// network-status backend to serve.
//
// Uses esbuild + esbuild-plugin-solid (both unscoped) rather than Vite +
// @vitejs/plugin-solid: the plugin is namespaced under @vitejs and is
// unreachable from this network's registry, so we keep the build toolchain
// to the minimum that resolves here.
//
// Output (into dist/):
//   bundle.js      the full app (solid-js + app code, minified ESM)
//   bundle.css     the stylesheet (inlined from src/styles.css)
//   index.html     the shell, referencing /bundle.js and /bundle.css
const esbuild = require("esbuild");
const fs = require("fs");
const path = require("path");
const { solidPlugin: solidFactory } = require("esbuild-plugin-solid");

const outDir = path.join(__dirname, "dist");
fs.rmSync(outDir, { recursive: true, force: true });

// Build without sourcemaps by default: the committed dist/ is a lean deploy
// asset (SOURCEMAP=true npm run build emits them for local debugging).
const sourceMap = process.env.SOURCEMAP === "true";

esbuild
  .build({
    entryPoints: [path.join("src", "main.tsx")],
    bundle: true,
    minify: true,
    sourcemap: sourceMap,
    format: "esm",
    target: ["es2020"],
    jsx: "automatic",
    jsxImportSource: "solid-js",
    outfile: path.join(outDir, "bundle.js"),
    plugins: [solidFactory()],
    logLevel: "info",
  })
  .then(() => {
    // Ship the HTML shell alongside the bundle (it references /bundle.js and
    // /bundle.css at the site root).
    const html = path.join(__dirname, "index.html");
    if (fs.existsSync(html)) {
      fs.copyFileSync(html, path.join(outDir, "index.html"));
    }
    console.log(`network-status-frontend: wrote ${path.relative(process.cwd(), outDir)}/`);
  })
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });
