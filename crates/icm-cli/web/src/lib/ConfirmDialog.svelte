<script lang="ts">
	import { confirmState, resolveConfirm } from './confirm.svelte';
</script>

{#if confirmState.current}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
		<div class="w-96 bg-[var(--card)] border border-[var(--border)] rounded-lg p-5 shadow-xl">
			<p class="text-sm mb-4">{confirmState.current.message}</p>
			<div class="flex justify-end gap-2">
				<button
					onclick={() => resolveConfirm(false)}
					class="px-3 py-1.5 text-sm rounded border border-[var(--border)] hover:bg-[var(--bg)] transition-colors"
				>
					Cancel
				</button>
				<button
					onclick={() => resolveConfirm(true)}
					class="px-3 py-1.5 text-sm rounded bg-[var(--accent)] hover:bg-[var(--accent-light)] text-white transition-colors"
				>
					Confirm
				</button>
			</div>
		</div>
	</div>
{/if}
