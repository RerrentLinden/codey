import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  base: "/codey/",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1421, strictPort: true },
});
