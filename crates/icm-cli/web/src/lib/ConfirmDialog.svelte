<script lang="ts">
	import { confirmState, resolveConfirm } from './confirm.svelte';

	let cancelBtn: HTMLButtonElement | undefined = $state();
	let confirmBtn: HTMLButtonElement | undefined = $state();
	// The element focused just before the dialog opened (the "delete" link
	// that triggered it, typically) — restored on close so keyboard users
	// land back where they were instead of at the top of the document.
	let previouslyFocused: HTMLElement | null = null;

	// A real bug found by a UX review, not a hypothetical: this dialog had
	// no focus management at all. Tabbing while it was open walked straight
	// through the *background* list underneath the backdrop — every row's
	// own "delete" button stayed focusable even though visually covered —
	// and pressing Enter on one silently swapped which memory the dialog
	// was about to delete, with only the dialog's own text (easy to miss)
	// as a clue. Moving focus in on open and trapping Tab inside the
	// dialog closes that off entirely.
	$effect(() => {
		if (confirmState.current) {
			previouslyFocused = document.activeElement as HTMLElement | null;
			// Cancel, not Confirm: this dialog is only ever used for
			// destructive actions (delete/prune/consolidate) — defaulting
			// focus to the safe choice means an accidental Enter press
			// doesn't do the destructive thing.
			cancelBtn?.focus();
		} else {
			previouslyFocused?.focus();
			previouslyFocused = null;
		}
	});

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			resolveConfirm(false);
			return;
		}
		if (e.key !== 'Tab') return;
		// Trap: the dialog has exactly two focusable elements, so wrapping
		// between them is the whole of the trap — no need for a generic
		// focusable-element query.
		e.preventDefault();
		if (e.shiftKey) {
			(document.activeElement === cancelBtn ? confirmBtn : cancelBtn)?.focus();
		} else {
			(document.activeElement === confirmBtn ? cancelBtn : confirmBtn)?.focus();
		}
	}
</script>

{#if confirmState.current}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
		role="dialog"
		aria-modal="true"
		aria-label="Confirm"
		onkeydown={onKeydown}
	>
		<div class="w-96 bg-[var(--card)] border border-[var(--border)] rounded-lg p-5 shadow-xl">
			<p class="text-sm mb-4">{confirmState.current.message}</p>
			<div class="flex justify-end gap-2">
				<button
					bind:this={cancelBtn}
					onclick={() => resolveConfirm(false)}
					class="px-3 py-1.5 text-sm rounded border border-[var(--border)] hover:bg-[var(--bg)] transition-colors"
				>
					Cancel
				</button>
				<button
					bind:this={confirmBtn}
					onclick={() => resolveConfirm(true)}
					class="px-3 py-1.5 text-sm rounded bg-[var(--accent)] hover:bg-[var(--accent-light)] text-white transition-colors"
				>
					Confirm
				</button>
			</div>
		</div>
	</div>
{/if}
