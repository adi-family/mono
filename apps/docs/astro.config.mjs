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
			customCss: ['./src/styles/wiki-link.css'],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/adi-family/mono' }],
			sidebar: [
				{
					label: 'Guides',
					items: [{ autogenerate: { directory: 'guides' } }],
				},
			],
		}),
	],
});
