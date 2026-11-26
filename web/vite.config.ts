import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    // One file each for the page's code and its styles: the binary serves them.
    modulePreload: { polyfill: false },
    assetsInlineLimit: 0,
  },
});
