import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
const runtimeEnv = (globalThis as typeof globalThis & {
  process?: { env?: Record<string, string | undefined> };
}).process?.env;
export default defineConfig({
  plugins: [svelte()],
  test: { include: ["src/**/*.test.ts"] },
  cacheDir: runtimeEnv?.BURRATA_VITE_CACHE_DIR ?? ".cache/vite",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/.cache/**", "**/src-tauri/target/**", "**/target/**"] },
    ...(runtimeEnv?.BURRATA_PROTECTED_FRONTEND === "1" ? { watch: null, hmr: false } : {}),
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: { target: "es2021" },
});
