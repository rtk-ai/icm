<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { api } from '$lib/api';
	import type { MemoirEntry } from '$lib/types';

	let memoirs: MemoirEntry[] = $state([]);
	let detail: any = $state(null);
	let selectedId: string | null = $state(null);
	let selectedNode: any = $state(null);
	let graphContainer: HTMLDivElement;
	let graphInstance: any = null;
	let viewMode: 'graph' | 'list' = $state('graph');

	// Relation → color mapping
	const RELATION_COLORS: Record<string, string> = {
		part_of: '#22c55e',
		depends_on: '#3b82f6',
		related_to: '#8b5cf6',
		contradicts: '#ef4444',
		refines: '#eab308',
		alternative_to: '#f97316',
		caused_by: '#06b6d4',
		instance_of: '#a3e635',
		superseded_by: '#6b7280',
	};

	onMount(async () => {
		memoirs = await api.memoirs();
	});

	async function showDetail(id: string) {
		selectedId = id;
		selectedNode = null;
		detail = await api.memoirDetail(id);
		await tick();
		if (viewMode === 'graph' && detail && browser) {
			renderGraph();
		}
	}

	async function renderGraph() {
		if (!graphContainer || !detail) return;

		// Dynamic import (Three.js is heavy, only load when needed)
		const ForceGraph3D = (await import('3d-force-graph')).default;

		// Build graph data
		const conceptMap = new Map<string, any>();
		for (const c of detail.concepts) {
			conceptMap.set(c.id, c);
		}

		const nodes = detail.concepts.map((c: any) => ({
			id: c.id,
			name: c.name,
			definition: c.definition,
			confidence: c.confidence,
			revision: c.revision,
			val: Math.max(2, c.confidence * 8), // node size
		}));

		const links = detail.links
			.filter((l: any) => conceptMap.has(l.source_id) && conceptMap.has(l.target_id))
			.map((l: any) => ({
				source: l.source_id,
				target: l.target_id,
				relation: l.relation,
				weight: l.weight,
				color: RELATION_COLORS[l.relation] || '#8b5cf6',
			}));

		// Clean up previous instance
		if (graphInstance) {
			graphInstance._destructor?.();
			graphContainer.innerHTML = '';
		}

		const width = graphContainer.clientWidth;
		const height = graphContainer.clientHeight;

		graphInstance = ForceGraph3D()(graphContainer)
			.width(width)
			.height(height)
			.backgroundColor('#0f172a')
			.nodeLabel((node: any) => `<div style="background:#1e293b;padding:6px 10px;border-radius:6px;border:1px solid #334155;font-size:12px;max-width:250px">
				<div style="font-weight:bold;color:#a78bfa">${node.name}</div>
				<div style="color:#94a3b8;margin-top:2px">${node.definition?.slice(0, 100) || ''}</div>
				<div style="color:#64748b;margin-top:4px;font-size:10px">confidence: ${node.confidence?.toFixed(2)} · rev: ${node.revision}</div>
			</div>`)
			.nodeColor((node: any) => {
				const conf = node.confidence || 0.5;
				if (conf >= 0.8) return '#22c55e';
				if (conf >= 0.5) return '#8b5cf6';
				return '#eab308';
			})
			.nodeOpacity(0.9)
			.nodeResolution(16)
			.linkColor((link: any) => link.color)
			.linkWidth((link: any) => Math.max(0.5, link.weight * 2))
			.linkOpacity(0.6)
			.linkDirectionalArrowLength(3.5)
			.linkDirectionalArrowRelPos(1)
			.linkLabel((link: any) => `<span style="background:#1e293b;padding:2px 6px;border-radius:4px;font-size:11px;color:${link.color}">${link.relation}</span>`)
			.onNodeClick((node: any) => {
				selectedNode = detail.concepts.find((c: any) => c.id === node.id) || null;
				// Focus camera on clicked node
				const distance = 40;
				const distRatio = 1 + distance / Math.hypot(node.x, node.y, node.z);
				graphInstance.cameraPosition(
					{ x: node.x * distRatio, y: node.y * distRatio, z: node.z * distRatio },
					node,
					1000
				);
			})
			.graphData({ nodes, links });

		// Initial camera position
		setTimeout(() => {
			graphInstance.zoomToFit(400);
		}, 500);
	}

	function switchView(mode: 'graph' | 'list') {
		viewMode = mode;
		if (mode === 'graph' && detail && browser) {
			tick().then(renderGraph);
		}
	}
</script>

<div class="flex items-center justify-between mb-4">
	<h2 class="text-2xl font-bold">Memoirs</h2>
	{#if detail}
		<div class="flex gap-1 bg-[var(--card)] rounded border border-[var(--border)] p-0.5">
			<button
				onclick={() => switchView('graph')}
				class="px-3 py-1 text-xs rounded transition-colors {viewMode === 'graph' ? 'bg-[var(--accent)] text-white' : 'text-[var(--muted)] hover:text-[var(--text)]'}"
			>
				3D Graph
			</button>
			<button
				onclick={() => switchView('list')}
				class="px-3 py-1 text-xs rounded transition-colors {viewMode === 'list' ? 'bg-[var(--accent)] text-white' : 'text-[var(--muted)] hover:text-[var(--text)]'}"
			>
				List
			</button>
		</div>
	{/if}
</div>

<div class="flex gap-4 h-[calc(100vh-8rem)]">
	<!-- Memoir list (left) -->
	<div class="w-64 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto flex-shrink-0">
		{#each memoirs as m}
			<button
				class="w-full text-left px-4 py-3 border-b border-[var(--border)] hover:bg-[var(--bg)] transition-colors
					{selectedId === m.id ? 'bg-[var(--bg)] border-l-2 border-l-[var(--accent)]' : ''}"
				onclick={() => showDetail(m.id)}
			>
				<div class="font-medium text-sm">{m.name}</div>
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

	<!-- Main panel -->
	<div class="flex-1 flex flex-col min-w-0">
		{#if detail}
			{#if viewMode === 'graph'}
				<!-- 3D Graph -->
				<div class="flex-1 bg-[var(--bg)] rounded-lg border border-[var(--border)] overflow-hidden relative">
					<div bind:this={graphContainer} class="w-full h-full"></div>

					<!-- Legend -->
					<div class="absolute bottom-3 left-3 bg-[var(--card)]/90 backdrop-blur rounded p-2 text-xs border border-[var(--border)]">
						<div class="font-medium mb-1 text-[var(--muted)]">Relations</div>
						{#each Object.entries(RELATION_COLORS) as [name, color]}
							<div class="flex items-center gap-1.5">
								<span class="w-3 h-0.5 rounded" style="background:{color}"></span>
								<span class="text-[var(--muted)]">{name}</span>
							</div>
						{/each}
					</div>

					<!-- Selected node info -->
					{#if selectedNode}
						<div class="absolute top-3 right-3 bg-[var(--card)] rounded-lg p-4 border border-[var(--border)] max-w-xs shadow-xl">
							<div class="flex items-center justify-between mb-2">
								<span class="font-bold text-[var(--accent-light)]">{selectedNode.name}</span>
								<button onclick={() => selectedNode = null} class="text-[var(--muted)] hover:text-[var(--text)]">x</button>
							</div>
							<p class="text-xs text-[var(--muted)] mb-2">{selectedNode.definition}</p>
							<div class="flex gap-3 text-xs">
								<span>confidence: <b>{selectedNode.confidence.toFixed(2)}</b></span>
								<span>rev: <b>{selectedNode.revision}</b></span>
							</div>
						</div>
					{/if}
				</div>
			{:else}
				<!-- List view -->
				<div class="flex-1 bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-auto p-4">
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
								<span style="color: {RELATION_COLORS[l.relation] || '#8b5cf6'}">{l.relation}</span>
								<span class="font-mono">{l.target_id.slice(0, 8)}</span>
								<span class="text-[var(--muted)] ml-auto">w={l.weight.toFixed(2)}</span>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		{:else}
			<div class="flex-1 bg-[var(--card)] rounded-lg border border-[var(--border)] flex items-center justify-center">
				<p class="text-[var(--muted)]">Select a memoir to visualize its knowledge graph</p>
			</div>
		{/if}
	</div>
</div>
