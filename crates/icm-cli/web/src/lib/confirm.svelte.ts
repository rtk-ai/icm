// A `window.confirm()` replacement that renders as an actual dashboard
// component (see ConfirmDialog.svelte, mounted once in the root layout)
// instead of an unstyleable native browser popup. Usage is the same shape
// as `confirm()` — `if (!(await askConfirm('Delete this?'))) return;` — so
// call sites barely change, they just gain an `await`.

interface ConfirmRequest {
	message: string;
	resolve: (ok: boolean) => void;
}

export const confirmState: { current: ConfirmRequest | null } = $state({ current: null });

export function askConfirm(message: string): Promise<boolean> {
	return new Promise(resolve => {
		confirmState.current = { message, resolve };
	});
}

export function resolveConfirm(ok: boolean) {
	confirmState.current?.resolve(ok);
	confirmState.current = null;
}
