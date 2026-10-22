import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `bun run dev` serves the UI with hot reload and sends API calls to a local
// `homie serve` on 127.0.0.1:7777.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:7777",
      "/auth": "http://127.0.0.1:7777",
    },
  },
  build: { outDir: "dist", emptyOutDir: true },
});
