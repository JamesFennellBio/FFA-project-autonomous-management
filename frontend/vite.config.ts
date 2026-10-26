import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// Blockly's images and cursors (zoom buttons, trash can) live in
// node_modules/blockly/media. The page may not load anything from the network,
// so they are served at /blockly-media/ in dev and copied into the build.
const TYPES: Record<string, string> = {
  svg: "image/svg+xml", png: "image/png", gif: "image/gif", cur: "image/x-icon", mp3: "audio/mpeg",
};

function blocklyMedia(): Plugin {
  const dir = join(dirname(createRequire(import.meta.url).resolve("blockly")), "media");
  return {
    name: "blockly-media",
    configureServer(server) {
      server.middlewares.use("/blockly-media", (req, res, next) => {
        const name = (req.url ?? "").replace(/^\//, "").split("?")[0];
        if (!/^[\w.-]+$/.test(name)) return next();
        try {
          const body = readFileSync(join(dir, name));
          const ext = name.split(".").pop() ?? "";
          res.setHeader("Content-Type", TYPES[ext] ?? "application/octet-stream");
          res.end(body);
        } catch {
          next();
        }
      });
    },
    generateBundle() {
      for (const name of readdirSync(dir)) {
        this.emitFile({ type: "asset", fileName: `blockly-media/${name}`, source: readFileSync(join(dir, name)) });
      }
    },
  };
}

export default defineConfig({
  plugins: [react(), blocklyMedia()],
  // Relative paths so the built page loads from file:// inside Electron.
  base: "./",
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true },
});
