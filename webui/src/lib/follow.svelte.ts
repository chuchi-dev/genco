/**
 * Keeps a growing log scrolled to its end, like a terminal: whatever
 * grows it (new text, a tool output, a loaded image) scrolls down while
 * the reader is at the end. Scrolling up stops it and new content sets
 * `unseen`, `down()` goes back to the end. `{@attach follow.attach}` on
 * the scrolling element.
 */
export class Follow {
	// the view sticks to the end
	stick = $state(true);
	// content came while the reader was scrolled up
	unseen = $state(false);

	#el: HTMLElement | null = null;
	#frame = 0;

	/** to the end and stay there, e.g. when a conversation opens */
	down() {
		this.stick = true;
		this.unseen = false;
		this.#schedule();
	}

	attach = (el: HTMLElement) => {
		this.#el = el;
		let lastTop = el.scrollTop;
		let height = el.scrollHeight;
		// no jumping while the reader selects text or drags the scrollbar
		let holding = false;

		const onscroll = () => {
			const end = el.scrollHeight - el.scrollTop - el.clientHeight;
			if (end < 40) {
				this.stick = true;
				this.unseen = false;
			} else if (el.scrollTop < lastTop) {
				// only the reader scrolls up, we and growing content don't
				this.stick = false;
			}
			lastTop = el.scrollTop;
		};

		const ongrow = () => {
			const grew = el.scrollHeight > height;
			height = el.scrollHeight;
			if (this.stick) {
				if (!holding) this.#schedule();
			} else if (grew) {
				this.unseen = true;
			}
		};

		const ondown = () => (holding = true);
		const onup = () => {
			if (!holding) return;
			holding = false;
			if (this.stick && !hasSelection(el)) this.#schedule();
		};

		// text and elements change, children resize when they render or
		// an image loads, the log itself when the window resizes
		const resize = new ResizeObserver(ongrow);
		resize.observe(el);
		const observeChildren = () => {
			for (const c of el.children) {
				resize.observe(c, { box: 'border-box' });
			}
		};
		observeChildren();
		const mutation = new MutationObserver(() => {
			observeChildren();
			ongrow();
		});
		mutation.observe(el, {
			childList: true,
			subtree: true,
			characterData: true,
		});

		el.addEventListener('scroll', onscroll, { passive: true });
		el.addEventListener('pointerdown', ondown);
		window.addEventListener('pointerup', onup);
		this.#schedule();

		return () => {
			resize.disconnect();
			mutation.disconnect();
			el.removeEventListener('scroll', onscroll);
			el.removeEventListener('pointerdown', ondown);
			window.removeEventListener('pointerup', onup);
			cancelAnimationFrame(this.#frame);
			this.#el = null;
		};
	};

	// after the layout of this frame
	#schedule() {
		cancelAnimationFrame(this.#frame);
		this.#frame = requestAnimationFrame(() => {
			const el = this.#el;
			if (el && this.stick) el.scrollTop = el.scrollHeight;
		});
	}
}

function hasSelection(el: HTMLElement): boolean {
	const s = getSelection();
	return !!s && !s.isCollapsed && el.contains(s.anchorNode);
}
