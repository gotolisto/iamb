// The front end of the UI host (architecture §10, ADR-0017). Vite serves it on a fixed port in
// development (`tauri dev` points the windows at it) and bundles it into `dist/` for release
// builds, which the Rust side embeds.
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  // The Tauri CLI prints its own output; keep its errors on screen.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Development only; the release build loads nothing from the network.
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // The system web views: WebView2 (Chromium) on Windows, WKWebView on macOS 13+.
    target: ["es2022", "chrome110", "safari16"],
    sourcemap: false,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
