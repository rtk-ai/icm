<script lang="ts">
	import { onMount } from 'svelte';
	import '../app.css';

	let { children } = $props();
	let collapsed = $state(false);
	let username = $state('');
	let version = $state('');

	const navItems = [
		{ href: '/', label: 'Overview', icon: 'O' },
		{ href: '/topics', label: 'Topics', icon: 'T' },
		{ href: '/memories', label: 'Memories', icon: 'M' },
		{ href: '/health', label: 'Health', icon: 'H' },
		{ href: '/memoirs', label: 'Memoirs', icon: 'K' },
		{ href: '/settings', label: 'Settings', icon: 'S' },
	];

	function toggleCollapse() {
		collapsed = !collapsed;
		try { localStorage.setItem('icm-sidebar-collapsed', String(collapsed)); } catch {}
	}

	function logout() {
		// Basic Auth logout: navigate to a URL with bad credentials to clear browser cache
		const url = new URL(window.location.href);
		url.username = 'logout';
		url.password = 'logout';
		window.location.href = url.toString();
	}

	onMount(async () => {
		try {
			const saved = localStorage.getItem('icm-sidebar-collapsed');
			if (saved === 'true') collapsed = true;
		} catch {}
		try {
			const res = await fetch('/api/whoami');
			if (res.ok) {
				const data = await res.json();
				username = data.username;
				version = data.version;
			}
		} catch {}
	});
</script>

<div class="flex h-screen">
	<!-- Sidebar -->
	<nav
		class="fixed top-0 left-0 bottom-0 z-40 flex flex-col border-r border-[var(--border)] bg-[var(--card)] overflow-y-auto transition-[width] duration-150"
		style="width: {collapsed ? '48px' : '200px'}"
	>
		<!-- Logo -->
		<a href="/" class="flex items-center border-b border-[var(--border)] {collapsed ? 'justify-center py-2 px-1' : 'px-3 py-3 gap-3'}">
			<img src="/icon.png" alt="ICM" class="h-8 w-8 rounded flex-shrink-0" />
			{#if !collapsed}
				<div>
					<div class="text-sm font-bold text-[var(--accent-light)]">ICM</div>
					<div class="text-[10px] text-[var(--muted)]">Infinite Context Memory</div>
				</div>
			{/if}
		</a>

		<!-- Nav -->
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

		<!-- User + Footer -->
		<div class="border-t border-[var(--border)]">
			{#if !collapsed}
				<!-- Connected user -->
				<div class="px-3 py-2 flex items-center justify-between">
					<div class="flex items-center gap-2">
						<span class="w-2 h-2 rounded-full bg-[var(--green)]"></span>
						<span class="text-xs text-[var(--text)]">{username}</span>
					</div>
					<button
						onclick={logout}
						title="Logout"
						class="text-xs text-[var(--muted)] hover:text-[var(--red)] transition-colors"
					>
						logout
					</button>
				</div>
			{:else}
				<div class="flex justify-center py-2">
					<span class="w-2 h-2 rounded-full bg-[var(--green)]" title="{username}"></span>
				</div>
			{/if}
			<!-- Version + collapse -->
			<div class="border-t border-[var(--border)] px-2 py-2 flex items-center {collapsed ? 'justify-center' : 'justify-between'}">
				{#if !collapsed}
					<span class="text-xs text-[var(--muted)]">v{version}</span>
				{/if}
				<button
					onclick={toggleCollapse}
					title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
					class="text-[var(--muted)] hover:text-[var(--text)] text-xs px-1"
				>
					{collapsed ? '>>' : '<<'}
				</button>
			</div>
		</div>
	</nav>

	<!-- Main -->
	<main
		class="flex-1 overflow-auto p-6 transition-[margin-left] duration-150"
		style="margin-left: {collapsed ? '48px' : '200px'}"
	>
		{@render children()}
	</main>
</div>
