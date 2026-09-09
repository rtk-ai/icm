// Backend auth is HTTP Basic (see web.rs's auth_middleware) — the browser's
// own native credential prompt works for it out of the box, but that's an
// ugly OS-chrome dialog with no way to style/brand it and no "invalid
// password" message, just a re-prompt loop. This module replaces that with
// a normal in-page login form: we still speak Basic Auth to the backend
// (no server changes needed), we just manage the Authorization header
// ourselves instead of letting the browser intercept a 401 and prompt.

const TOKEN_KEY = 'icm-dashboard-auth';

function hasStorage(): boolean {
	return typeof sessionStorage !== 'undefined';
}

/** The `Basic <base64>` token, or null if not logged in this tab session. */
export function getToken(): string | null {
	return hasStorage() ? sessionStorage.getItem(TOKEN_KEY) : null;
}

export function setCredentials(username: string, password: string): void {
	if (!hasStorage()) return;
	sessionStorage.setItem(TOKEN_KEY, btoa(`${username}:${password}`));
}

export function clearCredentials(): void {
	if (hasStorage()) sessionStorage.removeItem(TOKEN_KEY);
}

export function isLoggedIn(): boolean {
	return getToken() !== null;
}

/** Verify a token actually works against the backend (not just "present"). */
export async function verifyToken(token: string): Promise<boolean> {
	const res = await fetch('/api/topics', { headers: { Authorization: `Basic ${token}` } });
	return res.ok;
}
