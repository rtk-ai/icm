<script lang="ts">
	let username = $state('');
	let password = $state('');
	let error = $state('');
	let loading = $state(false);

	async function handleLogin(e: Event) {
		e.preventDefault();
		loading = true;
		error = '';
		try {
			const res = await fetch('/api/login', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ username, password }),
			});
			const data = await res.json();
			if (data.ok) {
				window.location.href = '/';
			} else {
				error = data.error || 'Login failed';
			}
		} catch {
			error = 'Connection error';
		}
		loading = false;
	}
</script>

<div class="min-h-screen flex items-center justify-center bg-[var(--bg)]">
	<div class="w-full max-w-sm">
		<div class="text-center mb-8">
			<img src="/icon.png" alt="ICM" class="w-16 h-16 mx-auto mb-4 rounded-lg" />
			<h1 class="text-2xl font-bold text-[var(--accent-light)]">ICM Dashboard</h1>
			<p class="text-sm text-[var(--muted)] mt-1">Infinite Context Memory</p>
		</div>

		<form onsubmit={handleLogin} class="bg-[var(--card)] rounded-lg border border-[var(--border)] p-6">
			{#if error}
				<div class="mb-4 px-3 py-2 bg-red-900/30 border border-red-700 rounded text-sm text-red-300">
					{error}
				</div>
			{/if}

			<div class="mb-4">
				<label for="username" class="block text-sm text-[var(--muted)] mb-1">Username</label>
				<input
					id="username"
					type="text"
					bind:value={username}
					required
					autofocus
					class="w-full bg-[var(--bg)] border border-[var(--border)] rounded px-3 py-2 text-sm focus:outline-none focus:border-[var(--accent)]"
				/>
			</div>

			<div class="mb-6">
				<label for="password" class="block text-sm text-[var(--muted)] mb-1">Password</label>
				<input
					id="password"
					type="password"
					bind:value={password}
					required
					class="w-full bg-[var(--bg)] border border-[var(--border)] rounded px-3 py-2 text-sm focus:outline-none focus:border-[var(--accent)]"
				/>
			</div>

			<button
				type="submit"
				disabled={loading}
				class="w-full py-2.5 bg-[var(--accent)] text-white rounded font-medium text-sm hover:bg-[var(--accent-light)] transition-colors disabled:opacity-50"
			>
				{loading ? 'Signing in...' : 'Sign in'}
			</button>
		</form>

		<p class="text-center text-xs text-[var(--muted)] mt-4">
			Secured by ICM
		</p>
	</div>
</div>
