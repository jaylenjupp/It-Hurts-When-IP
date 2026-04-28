import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  root: "src",
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
  },
});