<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';

	let { children } = $props();
	let collapsed = $state(false);

	const navItems = [
		{ href: '/', label: 'Overview', icon: 'O' },
		{ href: '/topics', label: 'Topics', icon: 'T' },
		{ href: '/memories', label: 'Memories', icon: 'M' },
		{ href: '/health', label: 'Health', icon: 'H' },
		{ href: '/memoirs', label: 'Memoirs', icon: 'K' },
	];

	function toggleCollapse() {
		collapsed = !collapsed;
		try { localStorage.setItem('icm-sidebar-collapsed', String(collapsed)); } catch {}
	}

	onMount(() => {
		try {
			const saved = localStorage.getItem('icm-sidebar-collapsed');
			if (saved === 'true') collapsed = true;
		} catch {}
	});
</script>

<div class="flex h-screen">
	<!-- Sidebar -->
	<nav
		class="fixed top-0 left-0 bottom-0 z-40 flex flex-col border-r border-[var(--border)] bg-[var(--card)] overflow-y-auto transition-[width] duration-150"
		style="width: {collapsed ? '48px' : '200px'}"
	>
		<!-- Logo header -->
		<div class="flex items-center border-b border-[var(--border)] {collapsed ? 'justify-center py-2' : 'px-3 py-3 gap-2'}">
			{#if collapsed}
				<img src="/icon.png" alt="ICM" class="w-8 h-8 rounded" />
			{:else}
				<img src="/banner.png" alt="ICM" class="h-8 rounded" />
			{/if}
		</div>

		<!-- Nav items -->
		<ul class="flex-1 py-2">
			{#each navItems as item}
				<li>
					<a
						href={item.href}
						class="flex items-center gap-3 py-2.5 text-sm hover:bg-[var(--bg)] transition-colors
							{collapsed ? 'justify-center px-0' : 'px-4'}"
						title={collapsed ? item.label : ''}
					>
						<span class="w-6 h-6 rounded bg-[var(--accent)] text-white text-xs flex items-center justify-center font-bold flex-shrink-0">
							{item.icon}
						</span>
						{#if !collapsed}
							{item.label}
						{/if}
					</a>
				</li>
			{/each}
		</ul>

		<!-- Footer with collapse toggle -->
		<div class="border-t border-[var(--border)] px-2 py-2 flex items-center {collapsed ? 'justify-center' : 'justify-between'}">
			{#if !collapsed}
				<span class="text-xs text-[var(--muted)]">v0.10.20</span>
			{/if}
			<button
				onclick={toggleCollapse}
				title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
				class="text-[var(--muted)] hover:text-[var(--text)] text-xs px-1"
			>
				{collapsed ? '>>' : '<<'}
			</button>
		</div>
	</nav>

	<!-- Main content -->
	<main
		class="flex-1 overflow-auto p-6 transition-[margin-left] duration-150"
		style="margin-left: {collapsed ? '48px' : '200px'}"
	>
		{@render children()}
	</main>
</div>
