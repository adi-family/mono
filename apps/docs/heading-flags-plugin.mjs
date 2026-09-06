// Bridges `buildHeadingFlagsMap()` (built once here, at `astro.config.mjs` load time, straight
// off disk — see `wiki-links.mjs`) into the Vite-bundled component graph as a virtual module.
//
// A plain `import` of `wiki-links.mjs` from a `.astro` component doesn't work: that component
// ends up bundled into `dist/.prerender/chunks/` at build time, and `wiki-links.mjs`'s `DOCS_DIR`
// (a `new URL('./src/content/docs/', import.meta.url)` relative to itself) then resolves relative
// to the *bundled chunk's* location instead of its real source location, and fails to find
// `src/content/docs` at all. Precomputing the map here, where `import.meta.url` is still this
// file's real location, and handing the plain result to Vite as static module content sidesteps
// that entirely.
import { buildHeadingFlagsMap } from './wiki-links.mjs';

const VIRTUAL_ID = 'virtual:doc-heading-flags';
const RESOLVED_ID = '\0' + VIRTUAL_ID;

export function headingFlagsPlugin() {
	const headingFlags = buildHeadingFlagsMap();
	return {
		name: 'adi-docs-heading-flags',
		resolveId(id) {
			if (id === VIRTUAL_ID) return RESOLVED_ID;
		},
		load(id) {
			if (id === RESOLVED_ID) {
				return `export const headingFlags = new Map(${JSON.stringify([...headingFlags])});`;
			}
		},
	};
}
