export interface Stats {
	total_memories: number;
	total_topics: number;
	avg_weight: number;
	oldest_memory: string | null;
	newest_memory: string | null;
	total_memoirs: number;
	total_concepts: number;
	total_links: number;
	total_feedback: number;
}

export interface TopicEntry {
	name: string;
	count: number;
}

export interface Memory {
	id: string;
	created_at: string;
	last_accessed: string;
	updated_at: string | null;
	access_count: number;
	weight: number;
	topic: string;
	summary: string;
	raw_excerpt: string | null;
	keywords: string[];
	importance: string;
	related_ids: string[];
}

export interface TopicHealth {
	topic: string;
	entry_count: number;
	avg_weight: number;
	avg_access_count: number;
	oldest: string | null;
	newest: string | null;
	last_accessed: string | null;
	needs_consolidation: boolean;
	stale_count: number;
}

export interface MemoirEntry {
	id: string;
	name: string;
	description: string;
	concepts: number;
	links: number;
}

export interface ActionResult {
	ok: boolean;
	message: string;
}
