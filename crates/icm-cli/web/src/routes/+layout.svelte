<script lang="ts">
	import '../app.css';

	let { children } = $props();

	const navItems = [
		{ href: '/', label: 'Overview', icon: 'O' },
		{ href: '/topics', label: 'Topics', icon: 'T' },
		{ href: '/memories', label: 'Memories', icon: 'M' },
		{ href: '/health', label: 'Health', icon: 'H' },
		{ href: '/memoirs', label: 'Memoirs', icon: 'K' },
	];
</script>

<div class="flex h-screen">
	<!-- Sidebar -->
	<nav class="w-52 bg-[var(--card)] border-r border-[var(--border)] flex flex-col">
		<div class="p-4 border-b border-[var(--border)]">
			<h1 class="text-lg font-bold text-[var(--accent-light)]">ICM Dashboard</h1>
			<p class="text-xs text-[var(--muted)]">Infinite Context Memory</p>
		</div>
		<ul class="flex-1 py-2">
			{#each navItems as item}
				<li>
					<a
						href={item.href}
						class="flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-[var(--bg)] transition-colors"
					>
						<span class="w-6 h-6 rounded bg-[var(--accent)] text-white text-xs flex items-center justify-center font-bold">
							{item.icon}
						</span>
						{item.label}
					</a>
				</li>
			{/each}
		</ul>
		<div class="p-3 border-t border-[var(--border)] text-xs text-[var(--muted)]">
			ICM v0.10.19
		</div>
	</nav>

	<!-- Main content -->
	<main class="flex-1 overflow-auto p-6">
		{@render children()}
	</main>
</div>
