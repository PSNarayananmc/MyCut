import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects the bundle at ../src-tauri/dist by default.
export default defineConfig({
  plugins: [react()],
  build: { outDir: "../src-tauri/dist", emptyOutDir: true },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
