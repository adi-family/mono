// `<Flags>Preview</Flags>` authored inside a heading compiles to an MDX JSX node whose slotted
// child is a real mdast text node — which is exactly what let a flag's name "ride along" into
// Astro's heading-text/anchor-id extraction (`@astrojs/markdown-remark`'s `rehype-collect-headings`
// walks a heading's `children` collecting text, and doesn't know or care that some of them came
// from a component). That's fine for a plain marker, but it means the flag's name ends up
// duplicated into the page's own `<h2>`/`<h3>` text and baked into its `id`.
//
// This plugin runs before that extraction (as a `remarkPlugins` entry, so on the mdast, before
// the mdast->hast conversion `rehype-collect-headings` sees) and moves each `<Flags>` child's
// text off the heading's `children` and onto a `kind` attribute instead: text collectors like
// `rehype-collect-headings` only ever visit `children`, never `attributes`, so a
// `<Flags kind="Preview" />` node contributes nothing to a heading's extracted text or slug.
// `Flags.astro` renders `kind` in place of its old slotted content, so nothing about the visible,
// in-body badge changes — only what leaks into the heading's own title and id.
//
// Emptying the `<Flags>` child isn't quite enough on its own: authoring `## Title <Flags>...`
// leaves a plain text mdast sibling immediately before it — `"Title "` — whose trailing space was
// only ever there to separate the words *visually*; mdast still carries it as part of the text.
// With the flag's own text gone, `rehype-collect-headings` collects that trailing space as the
// last character of the heading's title, and `github-slugger` turns a trailing space into a
// trailing hyphen (`"Title "` -> `"title-"`), which nothing else in the site ever produces or
// expects — `wiki-links.mjs`'s `headingsFor()` trims before slugging, so a `[[Page#Title]]` link
// disagrees with the id actually rendered. So: also strip the trailing whitespace off the
// preceding text node here, at the source, rather than trimming on every consumer. The visible
// gap before the badge is restored in `Flags.astro`'s own styling, not by leaving it in the text.
import { visit } from 'unist-util-visit';

function textOf(node) {
	let out = '';
	for (const child of node.children ?? []) {
		out += child.type === 'text' ? child.value : textOf(child);
	}
	return out;
}

export function remarkFlags() {
	return (tree) => {
		visit(tree, 'heading', (heading) => {
			heading.children.forEach((child, index) => {
				if (child.type !== 'mdxJsxTextElement' || child.name !== 'Flags') return;
				child.attributes = [{ type: 'mdxJsxAttribute', name: 'kind', value: textOf(child).trim() }];
				child.children = [];
				const prev = heading.children[index - 1];
				if (prev?.type === 'text') prev.value = prev.value.replace(/\s+$/, '');
			});
		});
	};
}
