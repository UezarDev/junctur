import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri loads the dev server at a fixed port (tauri.conf.json devUrl) and the built page from dist/.
export default defineConfig({
  root: "ui",
  plugins: [svelte({ configFile: "../svelte.config.js" })],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "../dist", emptyOutDir: true, target: "es2022" },
});
