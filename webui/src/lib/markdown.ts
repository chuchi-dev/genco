// The markdown the agent writes, as a tree the chat renders with plain
// elements: no HTML from the text ever reaches the page (it may quote
// logs and sites). Paragraphs, headings, lists, code blocks, `code`,
// **bold**, *italic* and [links](https://…) or to the panel's pages
// ([the password](/passwords/7)), nothing else (no javascript:).
// No text of the source is dropped: a proposal is approved as the admin
// reads it, so what follows a code fence stays as the block's label and a
// fence line with more text isn't a closing one.

export type Inline =
	| { t: 'text'; text: string }
	| { t: 'code'; text: string }
	| { t: 'b' | 'i'; children: Inline[] }
	/** `internal`: a page of the panel like /groups/3 or /passwords/7 */
	| { t: 'a'; href: string; internal: boolean; children: Inline[] };

export type Block =
	| { t: 'p'; inline: Inline[] }
	| { t: 'h'; inline: Inline[] }
	| {
			t: 'list';
			ordered: boolean;
			// nested items count on their own: `n` is the number among
			// their siblings, `ordered` their own kind
			items: {
				depth: number;
				ordered: boolean;
				n: number;
				inline: Inline[];
			}[];
	  }
	/** `label`: the text after the opening fence, like `bash` */
	| { t: 'code'; label: string; text: string };

const LIST = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;

export function parse(md: string): Block[] {
	const lines = md.replace(/\r/g, '').split('\n');
	const blocks: Block[] = [];
	let para: string[] = [];

	const flush = () => {
		if (para.length)
			blocks.push({ t: 'p', inline: inline(para.join('\n')) });
		para = [];
	};

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i];

		if (line.trimStart().startsWith('```')) {
			flush();
			const label = line.trim().slice(3).trim();
			const code: string[] = [];
			i++;
			while (i < lines.length && lines[i].trim() !== '```') {
				code.push(lines[i]);
				i++;
			}
			blocks.push({ t: 'code', label, text: code.join('\n') });
			continue;
		}

		const heading = /^#{1,6}\s+(.*)$/.exec(line);
		if (heading) {
			flush();
			blocks.push({ t: 'h', inline: inline(heading[1]) });
			continue;
		}

		const item = LIST.exec(line);
		if (item) {
			flush();
			const ordered = /\d/.test(item[2]);
			const depth = Math.floor(item[1].replace(/\t/g, '  ').length / 2);
			const last = blocks.at(-1);
			// a list may start at another number, like a continued one
			const n = ordered ? parseInt(item[2]) : 1;
			const entry = { depth, ordered, n, inline: inline(item[3]) };
			if (last?.t === 'list' && (last.ordered === ordered || depth > 0)) {
				// the item before on the same depth, unless a shallower one
				// came in between
				for (const before of last.items.toReversed()) {
					if (before.depth < depth) break;
					if (before.depth === depth) {
						if (before.ordered === ordered) entry.n = before.n + 1;
						break;
					}
				}
				last.items.push(entry);
			} else {
				blocks.push({ t: 'list', ordered, items: [entry] });
			}
			continue;
		}

		// an indented line goes on with the list item above it
		const last = blocks.at(-1);
		if (!para.length && last?.t === 'list' && /^\s+\S/.test(line)) {
			last.items
				.at(-1)!
				.inline.push({ t: 'text', text: ' ' }, ...inline(line.trim()));
			continue;
		}

		if (!line.trim()) flush();
		else para.push(line);
	}
	flush();
	return blocks;
}

export function inline(text: string): Inline[] {
	const out: Inline[] = [];
	let plain = '';
	const push = (node: Inline) => {
		if (plain) out.push({ t: 'text', text: plain });
		plain = '';
		out.push(node);
	};

	let i = 0;
	while (i < text.length) {
		const rest = text.slice(i);

		const code = /^`([^`\n]+)`/.exec(rest);
		if (code) {
			push({ t: 'code', text: code[1] });
			i += code[0].length;
			continue;
		}

		const bold = /^\*\*(?=\S)([\s\S]*?\S)\*\*/.exec(rest);
		if (bold) {
			push({ t: 'b', children: inline(bold[1]) });
			i += bold[0].length;
			continue;
		}

		// not inside words: snake_case and 2*3 stay as they are
		const before = text[i - 1] ?? ' ';
		const italic = /^([*_])(?=\S)([^*_\n]*?\S)\1(?![\w*])/.exec(rest);
		if (italic && !/\w/.test(before)) {
			push({ t: 'i', children: inline(italic[2]) });
			i += italic[0].length;
			continue;
		}

		const link =
			/^\[([^\]\n]+)\]\((https?:\/\/[^)\s]+|\/(?![\/\\])[^)\s\\]*)\)/.exec(
				rest,
			);
		if (link) {
			push({
				t: 'a',
				href: link[2],
				internal: link[2].startsWith('/'),
				children: inline(link[1]),
			});
			i += link[0].length;
			continue;
		}

		plain += text[i];
		i++;
	}
	if (plain) out.push({ t: 'text', text: plain });
	return out;
}
