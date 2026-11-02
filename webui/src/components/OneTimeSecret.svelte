<script lang="ts">
	// a token (or a command with one) that the server shows this once
	let {
		value,
		label = 'Copy it now, it is shown only once:',
	}: { value: string; label?: string } = $props();
</script>

<p><b>{label}</b></p>
<pre class="code selectable">{value}</pre>
