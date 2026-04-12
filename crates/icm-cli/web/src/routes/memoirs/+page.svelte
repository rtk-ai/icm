<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { MemoirEntry } from '$lib/types';

	let memoirs: MemoirEntry[] = $state([]);
	let detail: any = $state(null);
	let selectedId: string | null = $state(null);

	onMount(async () => {
		memoirs = await api.memoirs();
	});

	async function showDetail(id: string) {
		selectedId = id;
		detail = await api.memoirDetail(id);
	}
</script>

<h2 class="text-2xl font-bold mb-6">Memoirs</h2>

<div class="flex gap-4 h-[calc(100vh-8rem)]">
	<!-- Memoir list -->
	<div class="w-80 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto">
		{#each memoirs as m}
			<button
				class="w-full text-left px-4 py-3 border-b border-[var(--border)] hover:bg-[var(--bg)] transition-colors
					{selectedId === m.id ? 'bg-[var(--bg)]' : ''}"
				onclick={() => showDetail(m.id)}
			>
				<div class="font-medium text-sm">{m.name}</div>
				<div class="text-xs text-[var(--muted)] mt-0.5">{m.description}</div>
				<div class="flex gap-3 mt-1 text-xs text-[var(--muted)]">
					<span>{m.concepts} concepts</span>
					<span>{m.links} links</span>
				</div>
			</button>
		{/each}
		{#if memoirs.length === 0}
			<p class="p-4 text-[var(--muted)] text-sm">No memoirs yet</p>
		{/if}
	</div>

	<!-- Detail panel -->
	<div class="flex-1 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto p-4">
		{#if detail}
			<h3 class="text-lg font-semibold mb-2">{detail.memoir.name}</h3>
			<p class="text-sm text-[var(--muted)] mb-4">{detail.memoir.description}</p>

			<h4 class="font-medium mb-2">Concepts ({detail.concepts.length})</h4>
			<div class="space-y-2 mb-6">
				{#each detail.concepts as c}
					<div class="p-3 bg-[var(--bg)] rounded border border-[var(--border)]">
						<div class="flex items-center gap-2">
							<span class="font-medium text-sm">{c.name}</span>
							<span class="text-xs text-[var(--muted)]">conf={c.confidence.toFixed(2)}</span>
							<span class="text-xs text-[var(--muted)]">rev={c.revision}</span>
						</div>
						<p class="text-xs text-[var(--muted)] mt-1">{c.definition}</p>
					</div>
				{/each}
			</div>

			<h4 class="font-medium mb-2">Links ({detail.links.length})</h4>
			<div class="space-y-1">
				{#each detail.links as l}
					<div class="text-xs p-2 bg-[var(--bg)] rounded flex items-center gap-2">
						<span class="font-mono">{l.source_id.slice(0, 8)}</span>
						<span class="text-[var(--accent-light)]">{l.relation}</span>
						<span class="font-mono">{l.target_id.slice(0, 8)}</span>
						<span class="text-[var(--muted)] ml-auto">w={l.weight.toFixed(2)}</span>
					</div>
				{/each}
			</div>
		{:else}
			<p class="text-[var(--muted)]">Select a memoir to view details</p>
		{/if}
	</div>
</div>
