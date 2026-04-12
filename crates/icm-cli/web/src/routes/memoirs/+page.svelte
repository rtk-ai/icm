<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { browser } from '$app/environment';
	import { api } from '$lib/api';
	import type { MemoirEntry } from '$lib/types';

	let memoirs: MemoirEntry[] = $state([]);
	let detail: any = $state(null);
	let selectedId: string | null = $state(null);
	let selectedNode: any = $state(null);
	let selectedLink: any = $state(null);
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
		selectedLink = null;
		detail = await api.memoirDetail(id);
		await tick();
		if (viewMode === 'graph' && detail && browser) {
			renderGraph();
		}
	}

	/** Count how many links connect to a concept */
	function linkDegree(conceptId: string, links: any[]): number {
		return links.filter((l: any) => l.source_id === conceptId || l.target_id === conceptId).length;
	}

	/** Find connected links for a concept */
	function connectedLinks(conceptId: string) {
		if (!detail) return { incoming: [], outgoing: [] };
		const conceptMap = new Map(detail.concepts.map((c: any) => [c.id, c]));
		const incoming = detail.links
			.filter((l: any) => l.target_id === conceptId)
			.map((l: any) => ({ ...l, sourceName: conceptMap.get(l.source_id)?.name || '?' }));
		const outgoing = detail.links
			.filter((l: any) => l.source_id === conceptId)
			.map((l: any) => ({ ...l, targetName: conceptMap.get(l.target_id)?.name || '?' }));
		return { incoming, outgoing };
	}

	async function renderGraph() {
		if (!graphContainer || !detail) return;

		const ForceGraph3D = (await import('3d-force-graph')).default;

		const conceptMap = new Map<string, any>();
		for (const c of detail.concepts) {
			conceptMap.set(c.id, c);
		}

		// Node size = f(link degree, confidence)
		// More connections + higher confidence = bigger node
		const nodes = detail.concepts.map((c: any) => {
			const degree = linkDegree(c.id, detail.links);
			const size = Math.max(3, (degree + 1) * 2 + c.confidence * 4);
			return {
				id: c.id,
				name: c.name,
				definition: c.definition,
				confidence: c.confidence,
				revision: c.revision,
				labels: c.labels,
				degree,
				val: size,
			};
		});

		const links = detail.links
			.filter((l: any) => conceptMap.has(l.source_id) && conceptMap.has(l.target_id))
			.map((l: any) => ({
				source: l.source_id,
				target: l.target_id,
				relation: l.relation,
				weight: l.weight,
				color: RELATION_COLORS[l.relation] || '#8b5cf6',
				sourceName: conceptMap.get(l.source_id)?.name || '?',
				targetName: conceptMap.get(l.target_id)?.name || '?',
			}));

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
			.nodeLabel((node: any) => `<div style="background:#1e293b;padding:8px 12px;border-radius:8px;border:1px solid #334155;font-size:12px;max-width:280px;box-shadow:0 4px 12px rgba(0,0,0,0.3)">
				<div style="font-weight:bold;color:#a78bfa;font-size:13px">${node.name}</div>
				<div style="color:#94a3b8;margin-top:4px;line-height:1.4">${node.definition?.slice(0, 150) || ''}</div>
				<div style="color:#64748b;margin-top:6px;font-size:10px">confidence: ${node.confidence?.toFixed(2)} · rev: ${node.revision} · ${node.degree} connections</div>
			</div>`)
			.nodeColor((node: any) => {
				const conf = node.confidence || 0.5;
				if (conf >= 0.8) return '#22c55e';
				if (conf >= 0.5) return '#8b5cf6';
				return '#eab308';
			})
			.nodeOpacity(0.9)
			.nodeResolution(20)
			.linkColor((link: any) => link.color)
			.linkWidth((link: any) => Math.max(0.8, link.weight * 2.5))
			.linkOpacity(0.6)
			.linkDirectionalArrowLength(4)
			.linkDirectionalArrowRelPos(1)
			.linkDirectionalParticles(1)
			.linkDirectionalParticleWidth(1.5)
			.linkDirectionalParticleSpeed(0.005)
			.linkLabel((link: any) => `<span style="background:#1e293b;padding:3px 8px;border-radius:4px;font-size:11px;color:${link.color};border:1px solid #334155">${link.sourceName} → ${link.relation} → ${link.targetName}</span>`)
			.onNodeClick((node: any) => {
				selectedLink = null;
				selectedNode = {
					...detail.concepts.find((c: any) => c.id === node.id),
					degree: node.degree,
				};
				const distance = 40;
				const distRatio = 1 + distance / Math.hypot(node.x, node.y, node.z);
				graphInstance.cameraPosition(
					{ x: node.x * distRatio, y: node.y * distRatio, z: node.z * distRatio },
					node,
					1000
				);
			})
			.onLinkClick((link: any) => {
				selectedNode = null;
				selectedLink = link;
			})
			.graphData({ nodes, links });

		setTimeout(() => graphInstance.zoomToFit(400), 500);
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
					<div class="absolute bottom-3 left-3 bg-[var(--card)]/90 backdrop-blur rounded-lg p-3 text-xs border border-[var(--border)]">
						<div class="font-medium mb-1.5 text-[var(--text)]">Relations</div>
						{#each Object.entries(RELATION_COLORS) as [name, color]}
							<div class="flex items-center gap-2 py-0.5">
								<span class="w-4 h-0.5 rounded" style="background:{color}"></span>
								<span class="text-[var(--muted)]">{name}</span>
							</div>
						{/each}
						<div class="mt-2 pt-2 border-t border-[var(--border)]">
							<div class="font-medium mb-1 text-[var(--text)]">Node size</div>
							<div class="text-[var(--muted)]">connections + confidence</div>
						</div>
					</div>

					<!-- Selected node detail panel -->
					{#if selectedNode}
						{@const conn = connectedLinks(selectedNode.id)}
						<div class="absolute top-3 right-3 bg-[var(--card)] rounded-lg border border-[var(--border)] max-w-sm shadow-2xl overflow-hidden">
							<!-- Header -->
							<div class="flex items-center justify-between px-4 py-3 border-b border-[var(--border)] bg-[var(--accent)]/10">
								<span class="font-bold text-[var(--accent-light)]">{selectedNode.name}</span>
								<button onclick={() => selectedNode = null} class="text-[var(--muted)] hover:text-[var(--text)] text-lg leading-none">&times;</button>
							</div>
							<!-- Body -->
							<div class="p-4 space-y-3 max-h-80 overflow-auto">
								<p class="text-sm text-[var(--muted)]">{selectedNode.definition}</p>

								<div class="grid grid-cols-3 gap-2 text-xs">
									<div class="bg-[var(--bg)] rounded p-2 text-center">
										<div class="font-bold text-[var(--green)]">{selectedNode.confidence.toFixed(2)}</div>
										<div class="text-[var(--muted)]">confidence</div>
									</div>
									<div class="bg-[var(--bg)] rounded p-2 text-center">
										<div class="font-bold">{selectedNode.revision}</div>
										<div class="text-[var(--muted)]">revision</div>
									</div>
									<div class="bg-[var(--bg)] rounded p-2 text-center">
										<div class="font-bold text-[var(--blue)]">{selectedNode.degree}</div>
										<div class="text-[var(--muted)]">links</div>
									</div>
								</div>

								{#if selectedNode.labels?.length > 0}
									<div class="flex gap-1 flex-wrap">
										{#each selectedNode.labels as label}
											<span class="text-[10px] px-1.5 py-0.5 rounded bg-[var(--bg)] text-[var(--muted)] border border-[var(--border)]">{label.namespace}:{label.value}</span>
										{/each}
									</div>
								{/if}

								<!-- Outgoing links -->
								{#if conn.outgoing.length > 0}
									<div>
										<div class="text-xs font-medium text-[var(--muted)] mb-1">Outgoing ({conn.outgoing.length})</div>
										{#each conn.outgoing as l}
											<div class="text-xs flex items-center gap-1 py-0.5">
												<span style="color:{RELATION_COLORS[l.relation] || '#8b5cf6'}">{l.relation}</span>
												<span class="text-[var(--text)]">→ {l.targetName}</span>
											</div>
										{/each}
									</div>
								{/if}

								<!-- Incoming links -->
								{#if conn.incoming.length > 0}
									<div>
										<div class="text-xs font-medium text-[var(--muted)] mb-1">Incoming ({conn.incoming.length})</div>
										{#each conn.incoming as l}
											<div class="text-xs flex items-center gap-1 py-0.5">
												<span class="text-[var(--text)]">{l.sourceName} →</span>
												<span style="color:{RELATION_COLORS[l.relation] || '#8b5cf6'}">{l.relation}</span>
											</div>
										{/each}
									</div>
								{/if}
							</div>
						</div>
					{/if}

					<!-- Selected link detail panel -->
					{#if selectedLink}
						<div class="absolute top-3 right-3 bg-[var(--card)] rounded-lg border border-[var(--border)] max-w-xs shadow-2xl overflow-hidden">
							<div class="flex items-center justify-between px-4 py-3 border-b border-[var(--border)]">
								<span class="font-bold text-sm" style="color:{selectedLink.color}">{selectedLink.relation}</span>
								<button onclick={() => selectedLink = null} class="text-[var(--muted)] hover:text-[var(--text)] text-lg leading-none">&times;</button>
							</div>
							<div class="p-4 space-y-2">
								<div class="text-xs">
									<span class="text-[var(--muted)]">From:</span>
									<span class="text-[var(--text)] font-medium ml-1">{selectedLink.sourceName}</span>
								</div>
								<div class="text-xs">
									<span class="text-[var(--muted)]">To:</span>
									<span class="text-[var(--text)] font-medium ml-1">{selectedLink.targetName}</span>
								</div>
								<div class="text-xs">
									<span class="text-[var(--muted)]">Weight:</span>
									<span class="text-[var(--text)] ml-1">{selectedLink.weight?.toFixed(2)}</span>
								</div>
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
							{@const deg = linkDegree(c.id, detail.links)}
							<div class="p-3 bg-[var(--bg)] rounded border border-[var(--border)]">
								<div class="flex items-center gap-2">
									<span class="font-medium text-sm">{c.name}</span>
									<span class="text-xs text-[var(--muted)]">conf={c.confidence.toFixed(2)}</span>
									<span class="text-xs text-[var(--muted)]">rev={c.revision}</span>
									<span class="text-xs text-[var(--blue)]">{deg} links</span>
								</div>
								<p class="text-xs text-[var(--muted)] mt-1">{c.definition}</p>
							</div>
						{/each}
					</div>

					<h4 class="font-medium mb-2">Links ({detail.links.length})</h4>
					<div class="space-y-1">
						{#each detail.links as l}
							{@const conceptMap = new Map(detail.concepts.map((c: any) => [c.id, c]))}
							<div class="text-xs p-2 bg-[var(--bg)] rounded flex items-center gap-2">
								<span class="text-[var(--text)]">{conceptMap.get(l.source_id)?.name || l.source_id.slice(0, 8)}</span>
								<span style="color: {RELATION_COLORS[l.relation] || '#8b5cf6'}">→ {l.relation} →</span>
								<span class="text-[var(--text)]">{conceptMap.get(l.target_id)?.name || l.target_id.slice(0, 8)}</span>
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
