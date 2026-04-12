<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { TopicHealth, ActionResult } from '$lib/types';

	let healthData: TopicHealth[] = $state([]);
	let message = $state('');

	onMount(async () => {
		healthData = await api.healthAll();
	});

	async function runDecay() {
		const r = await api.decay();
		message = r.message;
		healthData = await api.healthAll();
	}

	async function runPrune() {
		if (!confirm('Prune all stale memories (weight < 0.1)?')) return;
		const r = await api.prune();
		message = r.message;
		healthData = await api.healthAll();
	}

	async function consolidate(topic: string) {
		if (!confirm(`Consolidate topic "${topic}"?`)) return;
		const r = await api.topicConsolidate(topic);
		message = r.message;
		healthData = await api.healthAll();
	}

	function fmtDate(d: string | null) {
		if (!d) return '-';
		return new Date(d).toLocaleDateString();
	}
</script>

<div class="flex items-center justify-between mb-6">
	<h2 class="text-2xl font-bold">Health</h2>
	<div class="flex gap-2">
		<button onclick={runDecay} class="px-3 py-1.5 bg-yellow-600 text-white rounded text-sm hover:bg-yellow-500">
			Decay All
		</button>
		<button onclick={runPrune} class="px-3 py-1.5 bg-red-600 text-white rounded text-sm hover:bg-red-500">
			Prune Stale
		</button>
	</div>
</div>

{#if message}
	<div class="mb-4 px-4 py-2 bg-[var(--card)] border border-[var(--accent)] rounded text-sm">
		{message}
	</div>
{/if}

<div class="bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto">
	<table class="w-full text-sm">
		<thead>
			<tr class="border-b border-[var(--border)] text-[var(--muted)]">
				<th class="text-left p-3">Topic</th>
				<th class="text-right p-3">Count</th>
				<th class="text-right p-3">Avg Weight</th>
				<th class="text-right p-3">Stale</th>
				<th class="text-center p-3">Consolidate?</th>
				<th class="text-left p-3">Last Access</th>
				<th class="text-center p-3">Actions</th>
			</tr>
		</thead>
		<tbody>
			{#each healthData as h}
				<tr class="border-b border-[var(--border)] hover:bg-[var(--bg)]">
					<td class="p-3">{h.topic}</td>
					<td class="p-3 text-right">{h.entry_count}</td>
					<td class="p-3 text-right">{h.avg_weight.toFixed(3)}</td>
					<td class="p-3 text-right {h.stale_count > 0 ? 'text-[var(--yellow)]' : ''}">{h.stale_count}</td>
					<td class="p-3 text-center">
						{#if h.needs_consolidation}
							<span class="text-[var(--yellow)]">Yes</span>
						{:else}
							<span class="text-[var(--green)]">No</span>
						{/if}
					</td>
					<td class="p-3 text-[var(--muted)]">{fmtDate(h.last_accessed)}</td>
					<td class="p-3 text-center">
						{#if h.needs_consolidation}
							<button
								onclick={() => consolidate(h.topic)}
								class="text-xs text-[var(--accent-light)] hover:underline"
							>consolidate</button>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
