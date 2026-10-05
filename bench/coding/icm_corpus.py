"""One sdebench task's corpus, laid out for `icm import`, and the question the
dry run asks of everything ICM injects: does this line come from the task's own
documents?

No upstream import: the documents are duck-typed (`id`, `messages`, `content`).

Import order. `icm import` reads a directory in file-name order, and several of
ICM's outputs select memories by insertion order rather than by search: the
SessionStart pack (the earliest high-importance memories of the project, then
the latest ones), `icm wake-up` and the `icm_wake_up` MCP tool (same pack),
`icm list`. The dataset has no dates, so the import order is the only "time"
ICM sees, and it is the adapter's choice. Imported in the dataset's order (the
task's own conversation first), or in two passes (every commit after every
conversation, which makes the decision commit one of "the latest"), the
document that carries the decision would reach the agent at session start
whatever the bug report says. So:

- conversations and commit messages go to ONE directory, imported in one pass;
- the other documents (decoys, host commits) are ranked by
  sha256(seed, document id): a keyed shuffle, the same on any machine;
- the task's own documents are placed, by the same hash, in the middle half of
  that order, never in the first or last quarter. A plain shuffle still puts
  one of them at an end now and then (measured: 5 of 183 task and seed pairs,
  each with a document among the first four or the last one), and that task
  would be answered by position. They keep the dataset's order among
  themselves: in an amended task the amending conversation stays after the one
  it amends.

The seed and the rank of the task's documents are recorded with the result, and
the dry run checks the outcome instead of trusting the construction: it replays
the SessionStart hook and counts the injected lines that come from the task's
own documents (`task_lines`). That count must be zero.

Labels. `icm import` stores the session id (conversations) or the file stem
(text files) as the memory's source. The dataset's ids say which conversation is
a decoy and which commit is the decision commit, so neither is written: files
and session ids carry the import rank only.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
from pathlib import Path

ORDER_SEED_DEFAULT = "icm-sde-1"
# Dataset ids of a task's own documents (memory_bench/dataset/sdebench.py at the
# pinned commit): `<task>:chat<n>` and `<task>:decision-commit`. Everything else
# in a task's corpus is shared noise (decoy conversations, host commits).
_TASK_DOC = re.compile(r":(chat\d+|decision-commit)$")
_ROLE_LABEL = re.compile(r"^\[(?:user|assistant)\]:\s*")
_CUT_MARK = re.compile(r"\s*\[…\]$")
# Shorter fragments ("No.", "Agreed.") match anything.
MIN_MATCH_CHARS = 16


def order_seed() -> str:
    return (os.environ.get("ICM_SDE_ORDER_SEED") or ORDER_SEED_DEFAULT).strip() or ORDER_SEED_DEFAULT


def rank_key(seed: str, doc_id: str) -> str:
    return hashlib.sha256(f"{seed}\n{doc_id}".encode()).hexdigest()


def is_task_document(doc_id: str) -> bool:
    return bool(_TASK_DOC.search(doc_id or ""))


def import_order(documents: list, seed: str) -> list:
    """The documents in the order `icm import` will read them (see the module docstring)."""
    own = [d for d in documents if is_task_document(d.id)]
    order = sorted((d for d in documents if not is_task_document(d.id)),
                   key=lambda d: rank_key(seed, d.id))
    total = len(documents)
    low, high = total // 4, total - total // 4 - len(own)
    span = max(high - low, 0) + 1
    # Slots are counted among the other documents; sorting them keeps the dataset's
    # order between the task's own documents.
    slots = sorted(low + int(rank_key(seed, d.id), 16) % span for d in own)
    for offset, (slot, doc) in enumerate(zip(slots, own)):
        order.insert(min(slot, len(order) - offset) + offset, doc)
    return order

def write_corpus(documents: list, corpus: Path, seed: str | None = None,
                 commits: bool = True) -> dict:
    """Write the files `icm import` reads and return what was written.

    Conversations (documents carrying `messages`) become Claude Code session
    files (`.jsonl`, one JSON object per turn). Everything else is a commit
    message and becomes a text file (`.md`); `commits=False` leaves those out.
    `icm import` picks the parser from the extension. The content is the
    dataset's, unchanged; the file names are the import rank.
    """
    seed = order_seed() if seed is None else seed
    shutil.rmtree(corpus, ignore_errors=True)
    corpus.mkdir(parents=True)

    kept = []
    for doc in documents:
        if doc.messages:
            if any(turn.get("content") for turn in doc.messages):
                kept.append(doc)
        elif commits and (doc.content or "").strip():
            kept.append(doc)

    written: list[tuple[str, str, str]] = []          # (document id, kind, file name)
    n_turns = 0
    for doc in import_order(kept, seed):
        stem = f"{len(written):04d}"
        if doc.messages:
            lines = []
            for turn in doc.messages:
                role = "assistant" if turn.get("role") == "assistant" else "user"
                text = turn.get("content") or ""
                if not text:
                    continue
                lines.append(json.dumps({"type": role, "session_id": stem,
                                         "message": {"role": role, "content": text}}))
            (corpus / f"{stem}.jsonl").write_text("\n".join(lines) + "\n")
            written.append((doc.id, "session", f"{stem}.jsonl"))
            n_turns += len(lines)
        else:
            (corpus / f"{stem}.md").write_text(doc.content.strip() + "\n")
            written.append((doc.id, "commit", f"{stem}.md"))

    task_documents = [
        {"id": doc_id, "kind": kind, "file": name, "rank": rank, "of": len(written)}
        for rank, (doc_id, kind, name) in enumerate(written)
        if is_task_document(doc_id)
    ]
    return {"sessions": sum(kind == "session" for _, kind, _ in written), "turns": n_turns,
            "commits": sum(kind == "commit" for _, kind, _ in written),
            "order_seed": seed, "task_documents": task_documents}

def task_texts(task: dict) -> list[str]:
    """Text of the task's own documents, as the dataset loader builds them: every
    turn of its conversation(s), and the decision commit of a history task."""
    conv = task.get("conversations") or []
    chats = conv if conv and isinstance(conv[0], list) else ([conv] if conv else [])
    texts = [turn.get("text") or "" for chat in chats for turn in chat]
    if task.get("decision_subject"):
        texts.append(f"Git commit: {task['decision_subject']}\n\n{task.get('decision_rationale', '')}")
    return [t for t in texts if t.strip()]


def document_texts(documents: list) -> list[str]:
    """The same texts, from the dataset's documents (the provider has no task file)."""
    texts = []
    for doc in documents:
        if not is_task_document(doc.id):
            continue
        if doc.messages:
            texts.extend(turn.get("content") or "" for turn in doc.messages)
        else:
            texts.append(doc.content or "")
    return [t for t in texts if t.strip()]


def _norm(text: str) -> str:
    return " ".join(text.split()).casefold()


def from_task(note: str, texts: list[str]) -> bool:
    """True when a stored note is a passage of the task's own documents.

    ICM's rule-based import stores sentences verbatim, sometimes behind a
    `[user]:` / `[assistant]:` label, and its renderers cut long ones with
    ` […]`. Both are removed before the note is looked up in the text.
    """
    body = _CUT_MARK.sub("", _ROLE_LABEL.sub("", note.strip()))
    needle = _norm(body)
    if len(needle) < MIN_MATCH_CHARS:
        return False
    return any(needle in _norm(text) for text in texts)


def task_lines(injected: str, texts: list[str]) -> list[str]:
    """The bullet lines of a hook's output that come from the task's own documents."""
    found = []
    for raw in (injected or "").splitlines():
        line = raw.strip()
        if line.startswith("- ") and from_task(line[2:], texts):
            found.append(line[2:])
    return found


def exported_task_facts(export_path: Path, texts: list[str]) -> list[str] | None:
    """Summaries of an `icm export` snapshot that come from the task's own
    documents, or None when the snapshot cannot be read."""
    try:
        rows = [json.loads(line) for line in export_path.read_text().splitlines() if line.strip()]
    except (OSError, ValueError):
        return None
    return [r["summary"] for r in rows
            if isinstance(r, dict) and r.get("type") == "memory" and from_task(r.get("summary") or "", texts)]
