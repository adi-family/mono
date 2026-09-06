// Obsidian-style [[Page Name]] / [[Page Name|Alias]] links, resolved against this project's own
// `src/content/docs` collection rather than a generic slugifier — so a link matches a page by
// its title *or* its file slug, case- and hyphenation-insensitively, without a separate index
// file to keep in sync.
//
// This has to run at `astro.config.mjs` load time, before Vite's content-collection pipeline
// exists, so it reads the docs directory straight off disk instead of importing `astro:content`.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { extname, join, relative } from 'node:path';
import GithubSlugger from 'github-slugger';

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

// Heading text, in document order, for the `Page#Section` wikilink form, alongside any `<Flags>`
// names that heading carries. This is a light approximation of how Astro itself derives a
// heading's visible text (see `@astrojs/markdown-remark`'s `rehype-collect-headings`) *after*
// `remark-flags.mjs` has run: that plugin moves a `<Flags>Bar</Flags>` child's text off the
// heading entirely (onto a `kind` attribute `visit`-based text collectors never look at), so a
// flagged heading's real text and id are the title alone. Strip `<Flags>...</Flags>` blocks
// *with* their content first, so a flag's own name isn't mistaken for part of the title, then
// strip any remaining inline HTML/JSX tags and simple emphasis/code markers, keeping their
// contents.
function headingsFor(file) {
	const body = readFileSync(file, 'utf-8').replace(/^---\r?\n[\s\S]*?\r?\n---/, '');
	const headings = [];
	for (const match of body.matchAll(/^#{1,6}\s+(.+)$/gm)) {
		const flags = [...match[1].matchAll(/<Flags>([^<]*)<\/Flags>/g)].map(([, name]) => name.trim());
		const text = match[1]
			.replace(/<Flags>[^<]*<\/Flags>/g, '')
			.replace(/<[^>]+>/g, '')
			.replace(/[*_`]/g, '')
			.replace(/\s+/g, ' ')
			.trim();
		if (text) headings.push({ text, flags });
	}
	return headings;
}

// One normalized key per title *and* per slug, both pointing at the real slug `hrefTemplate`
// needs — a link can name either. Also one normalized `page#section` key per heading, pointing
// at `slug#anchor-id` — the anchor id comes from `github-slugger`, the same slugger
// `rehype-collect-headings` uses, run in the same document order, so it matches the id Starlight
// actually renders. Matching itself still goes through `normalize()`, not the slugger, so a
// `[[Page#Some Heading]]` reference doesn't have to guess at slugger's punctuation handling.
function buildPermalinkMap() {
	const map = new Map();
	for (const file of collectDocFiles(DOCS_DIR)) {
		const slug = slugFor(file);
		map.set(normalize(slug || 'index'), slug);
		const title = titleFor(file);
		if (title) map.set(normalize(title), slug);

		const slugger = new GithubSlugger();
		for (const heading of headingsFor(file)) {
			const anchor = slugger.slug(heading.text);
			const target = `${slug}#${anchor}`;
			map.set(normalize(`${slug || 'index'}#${heading.text}`), target);
			if (title) map.set(normalize(`${title}#${heading.text}`), target);
		}
	}
	return map;
}

// slug::title -> ordered flag names for that heading, read off disk the same way as
// `permalinkMap` above — once, before content collections exist — so the table-of-contents
// override (`src/components/toc/`) can look up a page's flags without re-parsing its MDX itself.
// Keyed on the heading's *title text*, not its anchor id: reproducing `github-slugger`'s exact
// output (it does not trim trailing hyphens left by trailing whitespace before a self-closing
// component, e.g. a lone `<Flags />`'s heading text ending "...section ") off disk is exactly
// the kind of thing that's fragile to approximate — Starlight's own `heading.text` at the
// reading side needs the same whitespace-collapse this file's `headingsFor()` already does
// before either side is compared, but nothing slugger-specific.
export function buildHeadingFlagsMap() {
	const map = new Map();
	for (const file of collectDocFiles(DOCS_DIR)) {
		const slug = slugFor(file);
		for (const heading of headingsFor(file)) {
			if (heading.flags.length > 0) map.set(`${slug}::${heading.text}`, heading.flags);
		}
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
		// `[[Page#Section]]` is a single candidate string here, `#` and all — buildPermalinkMap
		// registered the composite key the same way, so the two either match whole or not at all.
		pageResolver: (name) => [normalize(name)],
		hrefTemplate: (permalink) => {
			const target = permalinkMap.get(permalink);
			if (target === undefined) return '#';
			const [slug, anchor] = target.split('#');
			const href = slug === '' ? base : `${base}${slug}/`;
			return anchor === undefined ? href : `${href}#${anchor}`;
		},
		wikiLinkClassName: 'wiki-link',
		newClassName: 'wiki-link-broken',
	};
}
