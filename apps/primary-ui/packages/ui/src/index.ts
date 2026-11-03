// Main entry. Importing it brings the Tailwind layer with it (extracted to dist/style.css by the
// build — import "@design-system/ui/style.css" in the consumer, or use the standalone build which
// inlines the CSS). Re-exports every component.
import "./styles.css";

export * from "./components";
