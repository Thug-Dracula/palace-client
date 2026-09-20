import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { svelteTesting } from "@testing-library/svelte/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  // `svelteTesting()` is inert outside Vitest (it keys off process.env.VITEST).
  // Under Vitest it resolves Svelte's browser build so components can be mounted
  // in jsdom, and adds the DOM auto-cleanup setup file.
  plugins: [sveltekit(), svelteTesting()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // base: "./" — required for the Tauri asset protocol: emitted chunk/wasm URLs
  // must resolve relative to the app root, not the page URL, or they 404 ("Load failed").
  base: "./",
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    // The libgme WASM glue lives at the project root (`wasm/gme/`), outside
    // Vite's default serving allow list — the app's initGme() dynamic import
    // and the `?url` wasm asset 403 without this. Allow the parent of the
    // Vite root (covers the whole project).
    fs: {
      allow: [".."],
    },
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
