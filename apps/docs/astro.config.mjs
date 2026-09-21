// @ts-check
import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import starlight from '@astrojs/starlight';
import rehypeMermaid from 'rehype-mermaid';
import wikiLinkPlugin from 'remark-wiki-link';
import { headingFlagsPlugin } from './heading-flags-plugin.mjs';
import { remarkFlags } from './remark-flags.mjs';
import { wikiLinkOptions } from './wiki-links.mjs';

// Served at the root of docs.withadi.dev.
const BASE = '/';

// https://astro.build/config
export default defineConfig({
	site: 'https://docs.withadi.dev',
	base: BASE,
	markdown: {
		// `remarkPlugins`/`rehypePlugins` on `markdown` directly are deprecated in Astro 7 in
		// favor of building the processor explicitly — see `markdown.processor` in the config
		// reference. `rehypeMermaid` defaults to `inline-svg`, rendering each ```mermaid fence
		// to a real `<svg>` at build time via a headless Chromium (mermaid-isomorphic +
		// playwright), not a client-shipped runtime.
		processor: unified({
			remarkPlugins: [[wikiLinkPlugin, wikiLinkOptions(BASE)], remarkFlags],
			rehypePlugins: [rehypeMermaid],
		}),
	},
	integrations: [
		starlight({
			title: 'ADI Mono',
			customCss: ['./src/styles/wiki-link.css', './src/styles/theme.css'],
			// Dark only, per design/DESIGN.md §3 — these two replace Starlight's default
			// dark/light toggle with a fixed dark theme; see the components themselves.
			// SiteTitle adds the ADI mark + wordmark (§10); Header adds the withadi.dev link
			// (no built-in labeled-link slot exists for it). TableOfContents/MobileTableOfContents
			// prepend each flagged heading's icon(s) to its sidebar entry — Starlight's own
			// `headings`/`toc` data only ever carries a heading's plain text, so there's no
			// prop-based way to do this without overriding the component itself; see
			// `src/components/toc/`.
			components: {
				ThemeProvider: './src/components/ThemeProvider.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
				SiteTitle: './src/components/SiteTitle.astro',
				Header: './src/components/Header.astro',
				TableOfContents: './src/components/toc/TableOfContents.astro',
				MobileTableOfContents: './src/components/toc/MobileTableOfContents.astro',
			},
			head: [
				// Geist / Geist Mono, loaded the same way design/examples/landing.html does.
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
				{
					tag: 'link',
					attrs: {
						rel: 'stylesheet',
						href: 'https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600&family=Geist+Mono:wght@400&display=swap',
					},
				},
				{ tag: 'meta', attrs: { name: 'theme-color', content: '#161616' } },
				{ tag: 'link', attrs: { rel: 'apple-touch-icon', sizes: '180x180', href: '/apple-touch-icon.png' } },
				// One site-wide social card (the mark + wordmark on --bg, see public/social-card.png) —
				// Starlight's own head has no og:image/twitter:image of its own, so the card was missing
				// entirely and every shared link rendered an empty or broken large-image card.
				{
					tag: 'meta',
					attrs: { property: 'og:image', content: 'https://docs.withadi.dev/social-card.png' },
				},
				{ tag: 'meta', attrs: { property: 'og:image:width', content: '1200' } },
				{ tag: 'meta', attrs: { property: 'og:image:height', content: '630' } },
				{
					tag: 'meta',
					attrs: { name: 'twitter:image', content: 'https://docs.withadi.dev/social-card.png' },
				},
			],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/adi-family/mono' }],
			sidebar: [
				{ label: 'Installation', link: '/installation/' },
				{
					label: 'Concepts',
					items: [
						{ label: 'Projects', link: '/projects/' },
						{ label: 'Agents', link: '/agents/' },
						{ label: 'Sessions', link: '/sessions/' },
						{ label: 'Tools', link: '/tools/' },
						{ label: 'Tasks', link: '/tasks/' },
						{ label: 'Triggers', link: '/triggers/' },
						{ label: 'Events', link: '/events/' },
						{ label: 'Secrets', link: '/secrets/' },
						{ label: 'Database', link: '/database/' },
						{ label: 'Knowledge', link: '/knowledge/' },
						{ label: 'Facts', link: '/facts/' },
						{ label: 'Hive', link: '/hive/' },
						{ label: 'Ports', link: '/ports/' },
						{ label: 'DNS and the front door', link: '/dns/' },
						{ label: 'Dashboards', link: '/dashboards/' },
						{ label: 'Marketplace', link: '/marketplace/' },
						{ label: 'Fleet', link: '/fleet/' },
						{ label: 'Mesh', link: '/mesh/' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'The adi-mono CLI', link: '/cli/' },
						{ label: 'Shared assets', link: '/shared-assets/' },
					],
				},
			],
		}),
	],
	vite: {
		plugins: [headingFlagsPlugin()],
	},
});
