import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Конфигурация Vite для работы в связке с Tauri:
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: [
        "**/target/**",
        "**/target",
        "**/src-tauri/**",
        "**/dist/**",
        "**/.biogit/**",
      ],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
    outDir: "dist",
  },
});