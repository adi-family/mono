// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import wikiLinkPlugin from 'remark-wiki-link';
import { wikiLinkOptions } from './wiki-links.mjs';

// Lives at docs.withadi.dev/mono/, alongside a sibling /cloud/ section added later.
const BASE = '/mono/';

// https://astro.build/config
export default defineConfig({
	site: 'https://docs.withadi.dev',
	base: BASE,
	markdown: {
		remarkPlugins: [[wikiLinkPlugin, wikiLinkOptions(BASE)]],
	},
	integrations: [
		starlight({
			title: 'ADI Mono',
			customCss: ['./src/styles/wiki-link.css', './src/styles/theme.css'],
			// Dark only, per design/DESIGN.md §3 — these two replace Starlight's default
			// dark/light toggle with a fixed dark theme; see the components themselves.
			// SiteTitle adds the ADI mark + wordmark (§10); Header adds the withadi.dev link
			// (no built-in labeled-link slot exists for it) — see the components themselves.
			components: {
				ThemeProvider: './src/components/ThemeProvider.astro',
				ThemeSelect: './src/components/ThemeSelect.astro',
				SiteTitle: './src/components/SiteTitle.astro',
				Header: './src/components/Header.astro',
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
			],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/adi-family/mono' }],
			sidebar: [
				{ label: 'Fleet', link: '/fleet/' },
				{
					label: 'Guides',
					items: [{ autogenerate: { directory: 'guides' } }],
				},
			],
		}),
	],
});
