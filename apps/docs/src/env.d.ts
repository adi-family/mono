/// <reference types="astro/client" />

// Starlight ships no ambient types for its virtual config/component modules — they only exist
// as Vite plugin output (@astrojs/starlight/dist/integrations/vite-virtual-modules.js). Needed
// because Header.astro's override re-composes Starlight's own sub-components, the same way its
// default does.
declare module 'virtual:starlight/user-config' {
	const config: import('@astrojs/starlight/types').StarlightConfig;
	export default config;
}

declare module 'virtual:starlight/components/*' {
	const Component: (...args: unknown[]) => unknown;
	export default Component;
}
