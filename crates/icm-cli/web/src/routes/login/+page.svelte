<script lang="ts">
	import { setCredentials, verifyToken, isLoggedIn } from '$lib/auth';

	let username = $state('admin');
	let password = $state('');
	let error = $state('');
	let checking = $state(false);

	if (typeof window !== 'undefined' && isLoggedIn()) {
		// Already logged in this tab (e.g. back-button to /login) — nothing
		// to do here.
		window.location.href = '/';
	}

	async function submit(e: Event) {
		e.preventDefault();
		error = '';
		checking = true;
		try {
			const token = btoa(`${username}:${password}`);
			const ok = await verifyToken(token);
			if (!ok) {
				error = 'Invalid username or password.';
				return;
			}
			setCredentials(username, password);
			const redirect = new URLSearchParams(window.location.search).get('redirect') || '/';
			window.location.href = redirect;
		} catch {
			error = 'Could not reach the ICM server.';
		} finally {
			checking = false;
		}
	}
</script>

<div class="min-h-screen flex items-center justify-center bg-[var(--bg)] text-[var(--text)]">
	<form
		onsubmit={submit}
		class="w-80 bg-[var(--card)] border border-[var(--border)] rounded-lg p-6 space-y-4"
	>
		<div>
			<h1 class="text-lg font-bold text-[var(--accent-light)]">ICM Dashboard</h1>
			<p class="text-xs text-[var(--muted)]">Sign in to continue</p>
		</div>

		{#if error}
			<div class="text-xs text-red-400 bg-red-400/10 border border-red-400/30 rounded px-3 py-2">
				{error}
			</div>
		{/if}

		<div class="space-y-1">
			<label for="username" class="text-xs text-[var(--muted)]">Username</label>
			<input
				id="username"
				name="username"
				type="text"
				autocomplete="username"
				bind:value={username}
				class="w-full bg-[var(--bg)] border border-[var(--border)] rounded px-3 py-2 text-sm focus:outline-none focus:border-[var(--accent)]"
			/>
		</div>

		<div class="space-y-1">
			<label for="password" class="text-xs text-[var(--muted)]">Password</label>
			<input
				id="password"
				name="password"
				type="password"
				autocomplete="current-password"
				bind:value={password}
				class="w-full bg-[var(--bg)] border border-[var(--border)] rounded px-3 py-2 text-sm focus:outline-none focus:border-[var(--accent)]"
			/>
		</div>

		<button
			type="submit"
			disabled={checking}
			class="w-full bg-[var(--accent)] hover:bg-[var(--accent-light)] disabled:opacity-50 text-white text-sm font-medium rounded px-3 py-2 transition-colors"
		>
			{checking ? 'Signing in…' : 'Sign in'}
		</button>
	</form>
</div>
