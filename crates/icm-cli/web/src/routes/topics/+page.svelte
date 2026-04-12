<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { TopicEntry, Memory } from '$lib/types';

	let topics: TopicEntry[] = $state([]);
	let selected: string | null = $state(null);
	let memories: Memory[] = $state([]);

	onMount(async () => {
		topics = await api.topics();
	});

	async function selectTopic(name: string) {
		selected = name;
		memories = await api.topicDetail(name);
	}

	function importanceBadge(imp: string) {
		const colors: Record<string, string> = {
			Critical: 'bg-red-600', High: 'bg-orange-500',
			Medium: 'bg-blue-500', Low: 'bg-gray-500'
		};
		return colors[imp] || 'bg-gray-500';
	}
</script>

<h2 class="text-2xl font-bold mb-6">Topics</h2>

<div class="flex gap-4 h-[calc(100vh-8rem)]">
	<!-- Topic list -->
	<div class="w-64 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto">
		{#each topics as topic}
			<button
				class="w-full text-left px-4 py-2.5 text-sm hover:bg-[var(--bg)] border-b border-[var(--border)] transition-colors
					{selected === topic.name ? 'bg-[var(--bg)] text-[var(--accent-light)]' : ''}"
				onclick={() => selectTopic(topic.name)}
			>
				<div class="flex justify-between">
					<span class="truncate">{topic.name}</span>
					<span class="text-[var(--muted)] ml-2">{topic.count}</span>
				</div>
			</button>
		{/each}
	</div>

	<!-- Memories list -->
	<div class="flex-1 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto p-4">
		{#if selected}
			<h3 class="text-lg font-semibold mb-4">{selected} ({memories.length})</h3>
			{#each memories as mem}
				<div class="mb-3 p-3 bg-[var(--bg)] rounded border border-[var(--border)]">
					<div class="flex items-center gap-2 mb-1">
						<span class="text-xs px-2 py-0.5 rounded {importanceBadge(mem.importance)} text-white">{mem.importance}</span>
						<span class="text-xs text-[var(--muted)]">w={mem.weight.toFixed(3)}</span>
						<span class="text-xs text-[var(--muted)]">x{mem.access_count}</span>
						<span class="text-xs text-[var(--muted)] ml-auto font-mono">{mem.id.slice(0, 12)}</span>
					</div>
					<p class="text-sm">{mem.summary}</p>
					{#if mem.keywords.length > 0}
						<div class="flex gap-1 mt-1 flex-wrap">
							{#each mem.keywords as kw}
								<span class="text-xs px-1.5 py-0.5 rounded bg-[var(--border)] text-[var(--muted)]">{kw}</span>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		{:else}
			<p class="text-[var(--muted)]">Select a topic to view its memories</p>
		{/if}
	</div>
</div>
