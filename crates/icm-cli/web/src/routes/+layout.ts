import { browser } from '$app/environment';
import { redirect } from '@sveltejs/kit';
import { isLoggedIn } from '$lib/auth';

// A SvelteKit `load` function, not a check inside +layout.svelte's onMount,
// on purpose: Svelte mounts children before their parent's onMount runs, so
// an onMount-based guard here would race a child page's own onMount (e.g.
// the graph page fetching data as soon as it mounts) — the child's fetch
// could fire, get a 401, and in Safari specifically that triggers the
// browser's native Basic Auth prompt before this guard ever gets a chance
// to redirect to the real login page. `load` runs before any component in
// the tree mounts, so there's no window for that race.
export const prerender = true;

export function load({ url }) {
	if (browser && url.pathname !== '/login' && !isLoggedIn()) {
		const redirectTo = `${url.pathname}${url.search}`;
		throw redirect(302, `/login?redirect=${encodeURIComponent(redirectTo)}`);
	}
}
