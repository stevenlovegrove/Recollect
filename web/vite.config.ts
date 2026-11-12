import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// `npm run dev` runs the engine in the browser, as the static site does. To work against
// `recollect serve` instead, set RECOLLECT_BACKEND to its address, such as http://127.0.0.1:7878.
const backend = process.env.RECOLLECT_BACKEND;

/** The page says it's served by the server, as the server's own copy of it does. */
const servedByServer: Plugin = {
  name: "recollect-backend",
  transformIndexHtml: (html) => html.replace("<head>", '<head>\n    <meta name="recollect-backend" content="http" />'),
};

export default defineConfig({
  plugins: [react(), ...(backend ? [servedByServer] : [])],
  // Relative, so the built site works from any path, such as a GitHub Pages project site.
  base: "./",
  worker: { format: "es" },
  // elkjs is ~1.4 MB and the engine ~2.6 MB; fine for an app.
  build: { chunkSizeWarningLimit: 2500 },
  server: {
    port: 5173,
    proxy: backend
      ? {
          "/api/events": { target: backend.replace(/^http/, "ws"), ws: true },
          "/api": backend,
          "/mcp": backend,
        }
      : undefined,
  },
});
