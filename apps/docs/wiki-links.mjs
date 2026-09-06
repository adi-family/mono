// Obsidian-style [[Page Name]] / [[Page Name|Alias]] links, resolved against this project's own
// `src/content/docs` collection rather than a generic slugifier — so a link matches a page by
// its title *or* its file slug, case- and hyphenation-insensitively, without a separate index
// file to keep in sync.
//
// This has to run at `astro.config.mjs` load time, before Vite's content-collection pipeline
// exists, so it reads the docs directory straight off disk instead of importing `astro:content`.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { extname, join, relative } from 'node:path';

const DOCS_DIR = new URL('./src/content/docs/', import.meta.url).pathname;

// Spaces, hyphens and underscores are the same word break to a person typing a wikilink, so
// "Getting Started", "getting-started" and "Getting_Started" must all resolve to one entry.
function normalize(name) {
	return name.trim().toLowerCase().replace(/[\s_-]+/g, '-');
}

function collectDocFiles(dir) {
	const files = [];
	for (const entry of readdirSync(dir)) {
		const full = join(dir, entry);
		if (statSync(full).isDirectory()) {
			files.push(...collectDocFiles(full));
		} else if (extname(entry) === '.md' || extname(entry) === '.mdx') {
			files.push(full);
		}
	}
	return files;
}

// Mirrors how Starlight derives a page's slug from its path: relative to the docs root, no
// extension, and an `index` file takes its parent directory's path (the root `index.mdx` becomes
// the empty slug, i.e. the site root).
function slugFor(file) {
	const rel = relative(DOCS_DIR, file).replace(/\.mdx?$/, '');
	return rel === 'index' ? '' : rel.replace(/\/index$/, '');
}

function titleFor(file) {
	const frontmatter = readFileSync(file, 'utf-8').match(/^---\r?\n([\s\S]*?)\r?\n---/)?.[1] ?? '';
	return frontmatter.match(/^title:\s*(.+)$/m)?.[1]?.trim().replace(/^["']|["']$/g, '');
}

// One normalized key per title *and* per slug, both pointing at the real slug `hrefTemplate`
// needs — a link can name either.
function buildPermalinkMap() {
	const map = new Map();
	for (const file of collectDocFiles(DOCS_DIR)) {
		const slug = slugFor(file);
		map.set(normalize(slug || 'index'), slug);
		const title = titleFor(file);
		if (title) map.set(normalize(title), slug);
	}
	return map;
}

// Returns just the plugin's options; astro.config.mjs pairs this with the plugin itself in an
// inline `[plugin, options]` tuple, because a tuple built here and returned would get widened to
// a plain array by TypeScript and fail `astro check` on the `remarkPlugins` entry's type.
//
// `base` is passed in rather than imported from astro.config.mjs, so this module has no cyclic
// dependency on the config that constructs it.
export function wikiLinkOptions(base) {
	const permalinkMap = buildPermalinkMap();
	return {
		aliasDivider: '|',
		permalinks: [...permalinkMap.keys()],
		pageResolver: (name) => [normalize(name)],
		hrefTemplate: (permalink) => {
			const slug = permalinkMap.get(permalink);
			if (slug === undefined) return '#';
			return slug === '' ? base : `${base}${slug}/`;
		},
		wikiLinkClassName: 'wiki-link',
		newClassName: 'wiki-link-broken',
	};
}
