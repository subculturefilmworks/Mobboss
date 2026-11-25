import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { buildInfo } from "../../scripts/build-info.mjs";
export default defineConfig({
  plugins: [react()],
  define: { __BUILD_INFO__: JSON.stringify(buildInfo()) },
  clearScreen: false,
});
