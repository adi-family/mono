import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';

// https://llmstxt.org — one line per page (title + description + absolute URL), generated from
// the same content collection Starlight builds the site from, so it can't drift from the sidebar.
// `draft: true` pages (see example.mdx) are excluded the same way Starlight excludes them from
// production routes and the sitemap.
const SITE = 'https://docs.withadi.dev';

export const GET: APIRoute = async () => {
	const pages = await getCollection('docs', ({ data }) => !data.draft);
	pages.sort((a, b) => a.id.localeCompare(b.id));

	const home = pages.find((page) => page.id === 'index');
	const rest = pages.filter((page) => page.id !== 'index');

	const lines = ['# ADI Docs', ''];
	if (home?.data.description) lines.push(`> ${home.data.description}`, '');
	lines.push('## Docs', '');

	for (const page of home ? [home, ...rest] : rest) {
		const url = `${SITE}/${page.id === 'index' ? '' : `${page.id}/`}`;
		const description = page.data.description ? `: ${page.data.description}` : '';
		lines.push(`- [${page.data.title}](${url})${description}`);
	}

	return new Response(lines.join('\n') + '\n', {
		headers: { 'Content-Type': 'text/plain; charset=utf-8' },
	});
};
