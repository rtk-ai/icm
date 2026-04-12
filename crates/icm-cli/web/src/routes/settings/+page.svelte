<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let configToml = $state('');
	let whoami = $state({ username: '', version: '' });
	let stats = $state<any>(null);

	onMount(async () => {
		const [configRes, whoamiRes, statsRes] = await Promise.all([
			fetch('/api/config').then(r => r.json()),
			fetch('/api/whoami').then(r => r.json()),
			api.stats(),
		]);
		configToml = configRes.config_toml || '# No config file found — using defaults';
		whoami = whoamiRes;
		stats = statsRes;
	});
</script>

<h2 class="text-2xl font-bold mb-6">Settings</h2>

<div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
	<!-- System Info -->
	<div class="bg-[var(--card)] rounded-lg border border-[var(--border)] p-5">
		<h3 class="text-lg font-semibold mb-4">System Info</h3>
		<div class="space-y-3 text-sm">
			<div class="flex justify-between">
				<span class="text-[var(--muted)]">Version</span>
				<span class="font-mono">v{whoami.version}</span>
			</div>
			<div class="flex justify-between">
				<span class="text-[var(--muted)]">Connected as</span>
				<span class="flex items-center gap-2">
					<span class="w-2 h-2 rounded-full bg-[var(--green)]"></span>
					{whoami.username}
				</span>
			</div>
			{#if stats}
				<div class="flex justify-between">
					<span class="text-[var(--muted)]">Total memories</span>
					<span>{stats.total_memories}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-[var(--muted)]">Total topics</span>
					<span>{stats.total_topics}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-[var(--muted)]">Memoirs</span>
					<span>{stats.total_memoirs}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-[var(--muted)]">Concepts / Links</span>
					<span>{stats.total_concepts} / {stats.total_links}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-[var(--muted)]">Avg weight</span>
					<span>{stats.avg_weight.toFixed(3)}</span>
				</div>
			{/if}
		</div>
	</div>

	<!-- Supported Tools -->
	<div class="bg-[var(--card)] rounded-lg border border-[var(--border)] p-5">
		<h3 class="text-lg font-semibold mb-4">Supported Tools (17)</h3>
		<div class="grid grid-cols-2 gap-1 text-xs">
			{#each [
				'Claude Code', 'Claude Desktop', 'Gemini CLI', 'Codex CLI',
				'Copilot CLI', 'Cursor', 'Windsurf', 'VS Code',
				'Amp', 'Amazon Q', 'Cline', 'Roo Code',
				'Kilo Code', 'Zed', 'OpenCode', 'Continue.dev', 'Aider'
			] as tool}
				<div class="flex items-center gap-1.5 py-0.5">
					<span class="w-1.5 h-1.5 rounded-full bg-[var(--green)]"></span>
					{tool}
				</div>
			{/each}
		</div>
	</div>
</div>

<!-- Config TOML -->
<div class="mt-6 bg-[var(--card)] rounded-lg border border-[var(--border)] p-5">
	<h3 class="text-lg font-semibold mb-3">Configuration (config.toml)</h3>
	<p class="text-xs text-[var(--muted)] mb-3">Read-only view of the active ICM configuration file.</p>
	<pre class="bg-[var(--bg)] rounded p-4 text-xs font-mono overflow-auto max-h-96 border border-[var(--border)] whitespace-pre-wrap">{configToml}</pre>
</div>
