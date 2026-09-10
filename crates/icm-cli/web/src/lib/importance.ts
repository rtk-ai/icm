// Shared importance -> color mapping. Was previously three separate
// copies (Memories, Topics, Graph), two of them keyed by capitalized
// strings ('Critical', 'High', ...) while the API actually returns
// lowercase ('critical', 'high', ...) — every lookup silently missed and
// fell through to the gray default, so importance color-coding was a
// no-op on both pages (UX review finding). One shared, lowercase-keyed
// map means a future new consumer can't reintroduce the same bug.
const TEXT_COLOR: Record<string, string> = {
	critical: 'text-red-400',
	high: 'text-orange-400',
	medium: 'text-blue-400',
	low: 'text-gray-400',
};

const BADGE_COLOR: Record<string, string> = {
	critical: 'bg-red-600',
	high: 'bg-orange-500',
	medium: 'bg-blue-500',
	low: 'bg-gray-500',
};

export function importanceTextColor(imp: string): string {
	return TEXT_COLOR[imp.toLowerCase()] ?? 'text-gray-400';
}

export function importanceBadgeColor(imp: string): string {
	return BADGE_COLOR[imp.toLowerCase()] ?? 'bg-gray-500';
}
