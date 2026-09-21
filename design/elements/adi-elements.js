// adi-elements — the adi design system as custom elements. One import, every tag.
//
//   <link rel="stylesheet" href="/design/tokens.css">
//   <script type="module" src="/elements/adi-elements.js"></script>
//   <adi-button variant="primary">Save</adi-button>
//
// No framework, no build step, no npm: these are HTMLElement subclasses and the browser is the
// runtime. The panel loads this file from `index.html` (Trunk copies the directory into `dist/`
// verbatim), which is why every element is available on every page of it, Leptos or not.
//
// What makes them look right is `design/tokens.css`, loaded by the page. Custom properties
// inherit through a shadow boundary, so the elements draw themselves out of whatever token file
// the host supplies and carry no colours of their own.
//
// The gallery rides along in this bundle rather than being fetched by the one page that shows
// it: it is a few KB beside a wasm panel, and a separate `<script>` tag for it would have to be
// injected from Rust to reach the same place.

export * from "./base.js";
export * from "./icon.js";
export * from "./button.js";
export * from "./field.js";
export * from "./segmented.js";
export * from "./tag.js";
export * from "./status.js";
export * from "./item.js";
export * from "./panel.js";
export * from "./table.js";
export * from "./kv.js";
export * from "./stat.js";
export * from "./code.js";
export * from "./notice.js";
export * from "./tool-call.js";
export * from "./modal.js";
export * from "./gallery.js";
