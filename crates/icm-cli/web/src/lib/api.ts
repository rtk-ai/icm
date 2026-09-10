import type { Stats, TopicEntry, Memory, TopicHealth, MemoirEntry, ActionResult, GraphResponse } from './types';
import { getToken, clearCredentials } from './auth';

const BASE = '/api';

function authHeaders(): HeadersInit {
	const token = getToken();
	return token ? { Authorization: `Basic ${token}` } : {};
}

/** On a real 401 (bad/expired credentials, not just "not logged in yet" —
 * the login page itself calls the backend directly, not through here),
 * drop the stale token and bounce to /login instead of leaving the app
 * silently broken. */
function handleUnauthorized() {
	clearCredentials();
	if (typeof window !== 'undefined' && window.location.pathname !== '/login') {
		window.location.href = '/login';
	}
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
	const res = await fetch(`${BASE}${path}`, { ...init, headers: { ...authHeaders(), ...init.headers } });
	if (res.status === 401) {
		handleUnauthorized();
		throw new Error('Unauthorized');
	}
	if (!res.ok) throw new Error(`HTTP ${res.status}`);
	return res.json();
}

const get = <T>(path: string) => request<T>(path);
const post = <T>(path: string) => request<T>(path, { method: 'POST' });
const del = <T>(path: string) => request<T>(path, { method: 'DELETE' });

export const api = {
	stats: () => get<Stats>('/stats'),
	topics: () => get<TopicEntry[]>('/topics'),
	topicDetail: (name: string) => get<Memory[]>(`/topics/${encodeURIComponent(name)}`),
	topicHealth: (name: string) => get<TopicHealth>(`/topics/${encodeURIComponent(name)}/health`),
	topicConsolidate: (name: string) => post<ActionResult>(`/topics/${encodeURIComponent(name)}/consolidate`),
	memories: (limit = 50, offset = 0) => get<Memory[]>(`/memories?limit=${limit}&offset=${offset}`),
	search: (q: string, limit = 20) => get<Memory[]>(`/memories/search?q=${encodeURIComponent(q)}&limit=${limit}`),
	deleteMemory: (id: string) => del<ActionResult>(`/memories/${id}`),
	healthAll: () => get<TopicHealth[]>('/health'),
	decay: () => post<ActionResult>('/health/decay'),
	prune: () => post<ActionResult>('/health/prune'),
	memoirs: () => get<MemoirEntry[]>('/memoirs'),
	memoirDetail: (id: string) => get<any>(`/memoirs/${id}`),
	graph: (topic?: string) => get<GraphResponse>(topic ? `/graph?topic=${encodeURIComponent(topic)}` : '/graph'),
};
