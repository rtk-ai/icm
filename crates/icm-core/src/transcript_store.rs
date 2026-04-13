use crate::error::IcmResult;
use crate::transcript::{Message, Role, Session, TranscriptHit, TranscriptStats};

/// Storage interface for verbatim transcripts (sessions + messages).
///
/// Unlike `Memory` (curated, decayed) and `Concept` (graph), a `Transcript`
/// is meant to be stored as-is — every user turn, every assistant reply,
/// every tool call. Retrieval filters at query time with FTS5.
pub trait TranscriptStore {
    /// Create a new session. Returns the session id.
    fn create_session(
        &self,
        agent: &str,
        project: Option<&str>,
        metadata: Option<&str>,
    ) -> IcmResult<String>;

    /// Fetch a session by id.
    fn get_session(&self, id: &str) -> IcmResult<Option<Session>>;

    /// List sessions, newest first. Optional project filter.
    fn list_sessions(&self, project: Option<&str>, limit: usize) -> IcmResult<Vec<Session>>;

    /// Record a message in an existing session. Returns the message id.
    /// Also bumps the session's `updated_at`.
    #[allow(clippy::too_many_arguments)]
    fn record_message(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        tool_name: Option<&str>,
        tokens: Option<i64>,
        metadata: Option<&str>,
    ) -> IcmResult<String>;

    /// List messages of a session in chronological order.
    fn list_session_messages(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> IcmResult<Vec<Message>>;

    /// Full-text search across messages (FTS5 BM25).
    /// `session_id` and `project` are optional narrowing filters.
    fn search_transcripts(
        &self,
        query: &str,
        session_id: Option<&str>,
        project: Option<&str>,
        limit: usize,
    ) -> IcmResult<Vec<TranscriptHit>>;

    /// Delete a session and cascade all its messages.
    fn forget_session(&self, id: &str) -> IcmResult<()>;

    /// Global transcript stats.
    fn transcript_stats(&self) -> IcmResult<TranscriptStats>;
}
