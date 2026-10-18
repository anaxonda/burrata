import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
export default defineConfig({
  root: "/tmp/burrata-downloads-ui-harness",
  plugins: [svelte()],
  server: { host: "127.0.0.1", port: 5189, strictPort: true, fs: { allow: ["/tmp/burrata-downloads-ui-harness", "/home/loki/projects/burrata"] } },
});
