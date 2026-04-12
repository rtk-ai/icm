<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Memory } from '$lib/types';

	let memories: Memory[] = $state([]);
	let query = $state('');
	let searching = $state(false);

	onMount(async () => {
		memories = await api.memories(100);
	});

	async function doSearch() {
		if (!query.trim()) {
			memories = await api.memories(100);
			return;
		}
		searching = true;
		memories = await api.search(query);
		searching = false;
	}

	async function deleteMemory(id: string) {
		if (!confirm(`Delete memory ${id.slice(0, 12)}...?`)) return;
		const result = await api.deleteMemory(id);
		if (result.ok) {
			memories = memories.filter(m => m.id !== id);
		}
	}

	function importanceColor(imp: string) {
		const colors: Record<string, string> = {
			Critical: 'text-red-400', High: 'text-orange-400',
			Medium: 'text-blue-400', Low: 'text-gray-400'
		};
		return colors[imp] || 'text-gray-400';
	}
</script>

<h2 class="text-2xl font-bold mb-6">Memories</h2>

<!-- Search bar -->
<div class="flex gap-2 mb-4">
	<input
		bind:value={query}
		onkeydown={(e) => e.key === 'Enter' && doSearch()}
		placeholder="Search memories (FTS5)..."
		class="flex-1 bg-[var(--card)] border border-[var(--border)] rounded px-3 py-2 text-sm focus:outline-none focus:border-[var(--accent)]"
	/>
	<button
		onclick={doSearch}
		class="px-4 py-2 bg-[var(--accent)] text-white rounded text-sm hover:bg-[var(--accent-light)] transition-colors"
	>
		{searching ? 'Searching...' : 'Search'}
	</button>
</div>

<!-- Memory list -->
<div class="space-y-2">
	{#each memories as mem}
		<div class="bg-[var(--card)] rounded-lg border border-[var(--border)] p-4">
			<div class="flex items-start gap-3">
				<div class="flex-1">
					<div class="flex items-center gap-2 mb-1">
						<span class="text-xs px-1.5 py-0.5 rounded bg-[var(--border)]">{mem.topic}</span>
						<span class="text-xs {importanceColor(mem.importance)}">{mem.importance}</span>
						<span class="text-xs text-[var(--muted)]">w={mem.weight.toFixed(3)}</span>
						<span class="text-xs text-[var(--muted)]">x{mem.access_count}</span>
					</div>
					<p class="text-sm mb-1">{mem.summary}</p>
					{#if mem.keywords.length > 0}
						<div class="flex gap-1 flex-wrap">
							{#each mem.keywords as kw}
								<span class="text-xs px-1.5 py-0.5 rounded bg-[var(--bg)] text-[var(--muted)]">{kw}</span>
							{/each}
						</div>
					{/if}
				</div>
				<div class="flex flex-col items-end gap-1">
					<span class="text-xs font-mono text-[var(--muted)]">{mem.id.slice(0, 12)}</span>
					<button
						onclick={() => deleteMemory(mem.id)}
						class="text-xs text-red-400 hover:text-red-300"
					>delete</button>
				</div>
			</div>
		</div>
	{/each}
</div>

{#if memories.length === 0}
	<p class="text-[var(--muted)] text-center mt-8">No memories found</p>
{/if}
