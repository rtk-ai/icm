<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Stats, TopicEntry } from '$lib/types';

	let stats: Stats | null = $state(null);
	let topics: TopicEntry[] = $state([]);

	onMount(async () => {
		[stats, topics] = await Promise.all([api.stats(), api.topics()]);
	});

	function fmtDate(d: string | null) {
		if (!d) return '-';
		return new Date(d).toLocaleDateString();
	}
</script>

<h2 class="text-2xl font-bold mb-6">Overview</h2>

{#if stats}
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold text-[var(--accent-light)]">{stats.total_memories}</div>
			<div class="text-sm text-[var(--muted)]">Memories</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold text-[var(--blue)]">{stats.total_topics}</div>
			<div class="text-sm text-[var(--muted)]">Topics</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold text-[var(--green)]">{stats.avg_weight.toFixed(2)}</div>
			<div class="text-sm text-[var(--muted)]">Avg Weight</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold text-[var(--yellow)]">{stats.total_memoirs}</div>
			<div class="text-sm text-[var(--muted)]">Memoirs</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold">{stats.total_concepts}</div>
			<div class="text-sm text-[var(--muted)]">Concepts</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold">{stats.total_links}</div>
			<div class="text-sm text-[var(--muted)]">Links</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-3xl font-bold">{stats.total_feedback}</div>
			<div class="text-sm text-[var(--muted)]">Feedback</div>
		</div>
		<div class="bg-[var(--card)] rounded-lg p-4 border border-[var(--border)]">
			<div class="text-sm font-mono">{fmtDate(stats.oldest_memory)}</div>
			<div class="text-sm font-mono">{fmtDate(stats.newest_memory)}</div>
			<div class="text-sm text-[var(--muted)]">Date Range</div>
		</div>
	</div>

	<h3 class="text-lg font-semibold mb-3">Top Topics</h3>
	<div class="bg-[var(--card)] rounded-lg border border-[var(--border)] p-4">
		{#each topics.slice(0, 15) as topic}
			<div class="flex items-center gap-3 mb-2">
				<span class="w-36 text-sm truncate">{topic.name}</span>
				<div class="flex-1 bg-[var(--bg)] rounded-full h-4 overflow-hidden">
					<div
						class="h-full bg-[var(--accent)] rounded-full transition-all"
						style="width: {Math.min(100, (topic.count / Math.max(...topics.map(t => t.count))) * 100)}%"
					></div>
				</div>
				<span class="text-sm text-[var(--muted)] w-8 text-right">{topic.count}</span>
			</div>
		{/each}
	</div>
{:else}
	<div class="text-[var(--muted)]">Loading...</div>
{/if}
