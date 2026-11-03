// Standalone entry: the whole library in one file with the CSS baked in. Importing it once (e.g.
// at the host app's root) injects the stylesheet as a <style> tag, so every page gets the styles
// without a separate import. Built by vite.standalone.config.ts into dist/standalone/.
import css from "./styles.css?inline";

if (typeof document !== "undefined" && !document.querySelector("style[data-design-system-ui]")) {
  const style = document.createElement("style");
  style.setAttribute("data-design-system-ui", "");
  style.textContent = css;
  document.head.appendChild(style);
}

export * from "./components";
