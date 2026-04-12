import type { Stats, TopicEntry, Memory, TopicHealth, MemoirEntry, ActionResult } from './types';

const BASE = '/api';

async function get<T>(path: string): Promise<T> {
	const res = await fetch(`${BASE}${path}`);
	if (!res.ok) throw new Error(`HTTP ${res.status}`);
	return res.json();
}

async function post<T>(path: string): Promise<T> {
	const res = await fetch(`${BASE}${path}`, { method: 'POST' });
	if (!res.ok) throw new Error(`HTTP ${res.status}`);
	return res.json();
}

async function del<T>(path: string): Promise<T> {
	const res = await fetch(`${BASE}${path}`, { method: 'DELETE' });
	if (!res.ok) throw new Error(`HTTP ${res.status}`);
	return res.json();
}

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
};
