import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  root: "/tmp/burrata-player-ui-harness",
  plugins: [svelte()],
  server: {
    host: "127.0.0.1",
    port: 5188,
    strictPort: true,
    fs: { allow: ["/tmp/burrata-player-ui-harness", "/home/loki/projects/burrata"] },
  },
});
