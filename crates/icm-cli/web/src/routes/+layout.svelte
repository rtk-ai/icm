<script lang="ts">
	import { page } from '$app/stores';
	import '../app.css';
	import ConfirmDialog from '$lib/ConfirmDialog.svelte';

	let { children } = $props();

	// The actual auth guard/redirect lives in +layout.ts's `load` function,
	// not here — it must run before any child page mounts and starts
	// fetching (see that file's comment for why an onMount-based check here
	// raced child pages in Safari). This derived value is UI-only: whether
	// to show the sidebar chrome around the login form.
	let isLoginPage = $derived($page.url.pathname === '/login');

	const navItems = [
		{ href: '/', label: 'Overview', icon: 'O' },
		{ href: '/topics', label: 'Topics', icon: 'T' },
		{ href: '/memories', label: 'Memories', icon: 'M' },
		{ href: '/health', label: 'Health', icon: 'H' },
		{ href: '/memoirs', label: 'Memoirs', icon: 'K' },
		{ href: '/graph', label: 'Graph', icon: 'G' },
	];

	const COLLAPSE_KEY = 'icm-dashboard-sidebar-collapsed';
	// Read synchronously at module init (not in onMount) so the sidebar
	// renders collapsed on first paint when that's the saved state, instead
	// of flashing open then snapping shut a frame later. A real mobile
	// width (~375px) with no saved preference used to always render the
	// full 208px expanded sidebar, leaving so little room for content
	// (~168px) that stat-card numbers and topic names were clipped mid-
	// character app-wide, not just wrapping awkwardly — worse than the
	// already-known header-wrapping issue at a wider ~900px. Defaulting
	// to collapsed below a real phone-width breakpoint, only when the
	// user hasn't already made an explicit choice, fixes the first-visit
	// case; the toggle still lets anyone expand it back.
	const MOBILE_BREAKPOINT = 640;
	let collapsed = $state(
		typeof localStorage !== 'undefined'
			? (localStorage.getItem(COLLAPSE_KEY) ?? (window.innerWidth < MOBILE_BREAKPOINT ? '1' : '0')) === '1'
			: false,
	);

	function toggleCollapsed() {
		collapsed = !collapsed;
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(COLLAPSE_KEY, collapsed ? '1' : '0');
		}
	}
</script>

{#if isLoginPage}
	{@render children()}
{:else}
<div class="flex h-screen">
	<!-- Sidebar -->
	<nav
		class="relative bg-[var(--card)] border-r border-[var(--border)] flex flex-col shrink-0 transition-[width] duration-150 {collapsed ? 'w-14' : 'w-52'}"
	>
		<button
			onclick={toggleCollapsed}
			title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
			aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
			class="absolute -right-3 top-5 w-6 h-6 rounded-full bg-[var(--card)] border border-[var(--border)] flex items-center justify-center text-[var(--muted)] hover:text-[var(--accent-light)] hover:border-[var(--accent)] transition-colors z-10"
		>
			<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="transition-transform {collapsed ? 'rotate-180' : ''}">
				<path d="M15 18l-6-6 6-6" stroke-linecap="round" stroke-linejoin="round" />
			</svg>
		</button>

		<div class="p-4 border-b border-[var(--border)] overflow-hidden">
			{#if collapsed}
				<h1 class="text-lg font-bold text-[var(--accent-light)] text-center">I</h1>
			{:else}
				<h1 class="text-lg font-bold text-[var(--accent-light)] whitespace-nowrap">ICM Dashboard</h1>
				<p class="text-xs text-[var(--muted)] whitespace-nowrap">Infinite Context Memory</p>
			{/if}
		</div>
		<ul class="flex-1 py-2">
			{#each navItems as item}
				<li>
					<a
						href={item.href}
						title={collapsed ? item.label : undefined}
						class="flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-[var(--bg)] transition-colors {collapsed ? 'justify-center px-0' : ''}"
					>
						<span class="w-6 h-6 rounded bg-[var(--accent)] text-white text-xs flex items-center justify-center font-bold shrink-0">
							{item.icon}
						</span>
						{#if !collapsed}
							<span class="whitespace-nowrap">{item.label}</span>
						{/if}
					</a>
				</li>
			{/each}
		</ul>
		<div class="p-3 border-t border-[var(--border)] text-xs text-[var(--muted)] whitespace-nowrap overflow-hidden">
			{collapsed ? 'v0.10.19' : 'ICM v0.10.19'}
		</div>
	</nav>

	<!-- Main content -->
	<main class="flex-1 overflow-auto p-6 min-w-0">
		{@render children()}
	</main>
</div>
{/if}

<ConfirmDialog />
