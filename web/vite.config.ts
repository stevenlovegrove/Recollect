import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const backend = process.env.RECOLLECT_BACKEND ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [react()],
  // elkjs is ~1.4 MB; fine for a local tool.
  build: { chunkSizeWarningLimit: 2500 },
  server: {
    port: 5173,
    proxy: {
      "/api/events": { target: backend.replace(/^http/, "ws"), ws: true },
      "/api": backend,
      "/mcp": backend,
    },
  },
});
