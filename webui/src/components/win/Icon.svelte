<script lang="ts" module>
	// small pixel icons in the style of Windows 95, drawn on a 16x16 grid
	export type IconName =
		| 'computer'
		| 'client'
		| 'group'
		| 'domain'
		| 'database'
		| 'agent'
		| 'users'
		| 'key'
		| 'gear'
		| 'file'
		| 'folder'
		| 'mail'
		| 'logo';
</script>

<script lang="ts">
	// `link` draws the arrow of a shortcut over the icon
	let {
		name,
		size = 16,
		link = false,
	}: { name: IconName; size?: number; link?: boolean } = $props();
</script>

<svg
	width={size}
	height={size}
	viewBox="0 0 16 16"
	shape-rendering="crispEdges"
	aria-hidden="true"
>
	{#if name === 'computer'}
		<rect x="2" y="1" width="12" height="10" fill="#c0c0c0" />
		<rect x="2" y="1" width="12" height="1" fill="#fff" />
		<rect x="3" y="2" width="10" height="7" fill="#000" />
		<rect x="4" y="3" width="8" height="5" fill="#008080" />
		<rect x="1" y="12" width="14" height="3" fill="#c0c0c0" />
		<rect x="1" y="14" width="14" height="1" fill="#808080" />
		<rect x="10" y="13" width="3" height="1" fill="#00a000" />
	{:else if name === 'client'}
		<rect x="5" y="1" width="6" height="6" fill="#ffcc99" />
		<rect x="5" y="1" width="6" height="2" fill="#804000" />
		<rect x="6" y="4" width="1" height="1" fill="#000" />
		<rect x="9" y="4" width="1" height="1" fill="#000" />
		<rect x="3" y="8" width="10" height="7" fill="#000080" />
		<rect x="7" y="8" width="2" height="4" fill="#fff" />
	{:else if name === 'group'}
		<rect x="1" y="3" width="6" height="2" fill="#c0a000" />
		<rect x="1" y="4" width="14" height="10" fill="#ffd800" />
		<rect x="1" y="13" width="14" height="1" fill="#806000" />
		<rect x="14" y="4" width="1" height="10" fill="#806000" />
		<rect x="9" y="8" width="5" height="5" fill="#0060c0" />
		<rect x="10" y="9" width="3" height="3" fill="#40c040" />
	{:else if name === 'domain'}
		<rect x="4" y="1" width="8" height="14" fill="#0060c0" />
		<rect x="2" y="3" width="12" height="10" fill="#0060c0" />
		<rect x="1" y="5" width="14" height="6" fill="#0060c0" />
		<rect x="4" y="4" width="4" height="3" fill="#40c040" />
		<rect x="9" y="8" width="4" height="4" fill="#40c040" />
		<rect x="5" y="10" width="2" height="2" fill="#40c040" />
		<rect x="4" y="2" width="3" height="1" fill="#80c0ff" />
	{:else if name === 'database'}
		<rect x="3" y="2" width="10" height="12" fill="#c0c0c0" />
		<rect x="4" y="1" width="8" height="1" fill="#c0c0c0" />
		<rect x="4" y="14" width="8" height="1" fill="#808080" />
		<rect x="3" y="2" width="10" height="2" fill="#fff" />
		<rect x="3" y="6" width="10" height="1" fill="#808080" />
		<rect x="3" y="10" width="10" height="1" fill="#808080" />
		<rect x="10" y="4" width="2" height="1" fill="#00a000" />
		<rect x="10" y="8" width="2" height="1" fill="#00a000" />
	{:else if name === 'mail'}
		<rect x="1" y="3" width="14" height="10" fill="#808080" />
		<rect x="2" y="4" width="12" height="8" fill="#ffffe0" />
		<rect x="3" y="5" width="2" height="1" fill="#808080" />
		<rect x="5" y="6" width="2" height="1" fill="#808080" />
		<rect x="7" y="7" width="2" height="1" fill="#808080" />
		<rect x="9" y="6" width="2" height="1" fill="#808080" />
		<rect x="11" y="5" width="2" height="1" fill="#808080" />
		<rect x="11" y="9" width="2" height="2" fill="#c00000" />
	{:else if name === 'agent'}
		<rect x="3" y="2" width="10" height="9" fill="#ffff80" />
		<rect x="2" y="3" width="12" height="7" fill="#ffff80" />
		<rect x="5" y="5" width="2" height="2" fill="#000" />
		<rect x="9" y="5" width="2" height="2" fill="#000" />
		<rect x="6" y="8" width="4" height="1" fill="#000" />
		<rect x="5" y="11" width="6" height="2" fill="#808080" />
		<rect x="6" y="13" width="4" height="2" fill="#404040" />
	{:else if name === 'users'}
		<rect x="2" y="2" width="5" height="5" fill="#ffcc99" />
		<rect x="1" y="8" width="7" height="6" fill="#008000" />
		<rect x="9" y="3" width="5" height="5" fill="#ffcc99" />
		<rect x="8" y="9" width="7" height="6" fill="#800000" />
	{:else if name === 'key'}
		<rect x="1" y="5" width="6" height="6" fill="#c0a000" />
		<rect x="3" y="7" width="2" height="2" fill="#fff" />
		<rect x="7" y="7" width="8" height="2" fill="#c0a000" />
		<rect x="11" y="9" width="2" height="3" fill="#c0a000" />
		<rect x="14" y="9" width="1" height="2" fill="#c0a000" />
	{:else if name === 'gear'}
		<rect x="6" y="1" width="4" height="14" fill="#808080" />
		<rect x="1" y="6" width="14" height="4" fill="#808080" />
		<rect x="3" y="3" width="10" height="10" fill="#808080" />
		<rect x="6" y="6" width="4" height="4" fill="#c0c0c0" />
	{:else if name === 'file'}
		<rect x="3" y="1" width="8" height="14" fill="#fff" />
		<rect x="11" y="4" width="2" height="11" fill="#fff" />
		<rect x="3" y="15" width="10" height="1" fill="#000" />
		<rect x="13" y="4" width="1" height="12" fill="#000" />
		<rect x="5" y="5" width="6" height="1" fill="#808080" />
		<rect x="5" y="8" width="6" height="1" fill="#808080" />
		<rect x="5" y="11" width="6" height="1" fill="#808080" />
	{:else if name === 'folder'}
		<rect x="1" y="3" width="6" height="2" fill="#c0a000" />
		<rect x="1" y="4" width="14" height="10" fill="#ffd800" />
		<rect x="1" y="13" width="14" height="1" fill="#806000" />
		<rect x="14" y="4" width="1" height="10" fill="#806000" />
		<rect x="1" y="6" width="14" height="1" fill="#fff080" />
	{:else if name === 'logo'}
		<rect x="4" y="1" width="8" height="14" fill="#c0c0c0" />
		<rect x="4" y="1" width="8" height="1" fill="#fff" />
		<rect x="11" y="1" width="1" height="14" fill="#404040" />
		<rect x="5" y="3" width="5" height="1" fill="#000080" />
		<rect x="5" y="5" width="5" height="1" fill="#000080" />
		<rect x="5" y="7" width="5" height="1" fill="#000080" />
		<rect x="8" y="11" width="2" height="2" fill="#00c000" />
	{/if}
	{#if link}
		<rect x="1" y="9" width="7" height="6" fill="#fff" />
		<rect x="1" y="9" width="7" height="1" fill="#000" />
		<rect x="1" y="14" width="7" height="1" fill="#000" />
		<rect x="3" y="11" width="3" height="1" fill="#000" />
		<rect x="5" y="10" width="1" height="3" fill="#000" />
	{/if}
</svg>
