//! Knowledge retention benchmark — measures memory accuracy, not just token savings.
//!
//! Session 1: agent reads dense factual content (in the prompt, not as files).
//! Sessions 2+: agent answers factual questions WITHOUT the source text.
//!
//! Without ICM: agent has no memory of session 1 → must guess or say "I don't know".
//! With ICM: agent stored facts in session 1 → can recall and answer accurately.

/// Dense factual document for session 1.
/// Fictional but internally consistent, with many specific testable facts.
pub const SOURCE_DOCUMENT: &str = r#"# Chapter 7: The Meridian Protocol — Distributed Consensus for Edge Networks

## 7.1 Origins

The Meridian Protocol was proposed by Dr. Elena Vasquez and Dr. Kenji Tanaka at SIGCOMM 2019 in Beijing. Their paper "Meridian: Sub-millisecond Consensus at the Edge" won Best Paper and introduced a three-phase commit protocol optimized for geo-distributed edge clusters.

The motivation came from Project Lighthouse at Arista Networks, where Vasquez observed that Raft consensus degraded to 12ms latency with nodes spread across more than 3 datacenters. Tanaka's prior work on gossip protocols (the "Firefly" protocol, published at SOSP 2017) provided the foundation for Meridian's peer discovery layer.

## 7.2 Protocol Design

Meridian uses a three-phase commit:

1. **Propose phase** (τ₁ = 150ms timeout): The elected leader broadcasts a proposal to all replicas. Proposals carry a monotonic epoch number starting at 1. Each proposal includes a Merkle root of the pending transaction batch.

2. **Validate phase** (τ₂ = 300ms timeout): Replicas verify the proposal against their local state. A replica votes YES if its local epoch is at most 1 behind the leader's epoch. The leader needs votes from ⌈(2n+1)/3⌉ replicas to proceed (where n is the cluster size).

3. **Commit phase** (τ₃ = 50ms timeout): The leader multicasts the commit certificate. Replicas apply the transaction batch atomically. If τ₃ expires without acknowledgment from a replica, it enters recovery mode.

The total worst-case latency for a single transaction is τ₁ + τ₂ + τ₃ = 500ms, but in practice the median latency is 47ms on a well-connected cluster.

## 7.3 Cluster Configuration

- **Maximum cluster size**: 127 nodes (limited by the 7-bit node ID in the header)
- **Minimum cluster size**: 5 nodes (to tolerate 1 Byzantine fault)
- **Gossip protocol port**: 9471 (UDP) for peer discovery, 9472 (TCP) for state sync
- **Leader election timeout**: 2000ms (4× the commit timeout)
- **Heartbeat interval**: 250ms
- **Maximum transaction batch size**: 64KB (configurable up to 1MB)
- **Epoch rollover**: after 2^48 epochs (approximately 8,900 years at 1000 TPS)

## 7.4 Fault Tolerance

Meridian tolerates up to f Byzantine faults in a cluster of n = 3f + 1 nodes. For crash-only faults, the requirement relaxes to n = 2f + 1.

The protocol uses a novel "witness chain" mechanism for Byzantine detection: each node maintains a hash chain of all messages it has sent in the current epoch. During the validate phase, nodes exchange witness chain tips. If a node detects a fork (two different messages with the same sequence number), it broadcasts a BLAME message that triggers leader rotation.

The BLAME threshold is f + 1 messages — once enough nodes report the same fork, the leader is blacklisted for 10 epochs.

## 7.5 Performance

Benchmark results from the original paper (2019), measured on AWS c5.4xlarge instances across 5 regions (us-east-1, eu-west-1, ap-northeast-1, us-west-2, sa-east-1):

| Cluster Size | Throughput (TPS) | Median Latency | P99 Latency |
|-------------|-----------------|----------------|-------------|
| 7 nodes     | 89,000          | 23ms           | 67ms        |
| 13 nodes    | 71,000          | 31ms           | 94ms        |
| 31 nodes    | 52,000          | 42ms           | 156ms       |
| 64 nodes    | 47,000          | 47ms           | 203ms       |
| 127 nodes   | 38,000          | 58ms           | 312ms       |

The throughput scales sub-linearly due to the O(n) message complexity in the validate phase. Vasquez's 2021 follow-up paper "Meridian-S" introduced a sharded variant that achieves near-linear scaling by partitioning the key space into √n shards.

## 7.6 Implementations

Three major implementations exist:
1. **libmeridian** (C++, 47,000 lines): The reference implementation by Vasquez's team at Stanford. Licensed under Apache 2.0.
2. **meridian-rs** (Rust, 12,000 lines): Community implementation by the Constellation Labs team. Uses tokio for async I/O. Licensed under MIT.
3. **PyMeridian** (Python, 3,200 lines): Educational implementation by Prof. Sarah Chen at MIT. Not suitable for production due to GIL limitations.

## 7.7 Real-World Deployments

- **Cloudflare Workers KV** (2020): Uses a modified Meridian for edge-consistent reads. Deployed across 285 PoPs. Reported 99.97% availability.
- **Akamai EdgeDB** (2021): Adopted Meridian-S for their distributed cache layer. Handles 2.3 million TPS peak.
- **Fastly Compute@Edge** (2022): Uses Meridian for their session store. Reduced consistency violations by 94% compared to their previous eventual consistency model.

## 7.8 Limitations

1. **Write amplification**: Each write is replicated to all n nodes, giving O(n) write amplification.
2. **No partial replication**: Every node stores the full state. Meridian-S addresses this with sharding.
3. **Leader bottleneck**: All writes go through the leader. Throughput is bounded by the leader's network bandwidth.
4. **Cold start**: A new node joining the cluster requires a full state transfer, which takes O(S/B) time where S is state size and B is bandwidth. For a 100GB state on a 10Gbps link, this is approximately 80 seconds.
"#;

/// Questions with expected answer keywords for scoring.
pub struct Question {
    pub prompt: &'static str,
    pub expected: &'static [&'static str],
    pub min_matches: usize,
}

pub const QUESTIONS: &[Question] = &[
    Question {
        prompt: "Who proposed the Meridian Protocol, at which conference, and in what year?",
        expected: &["Vasquez", "Tanaka", "SIGCOMM", "2019", "Beijing"],
        min_matches: 3,
    },
    Question {
        prompt: "What are the three phases of Meridian and their timeouts?",
        expected: &["Propose", "150", "Validate", "300", "Commit", "50"],
        min_matches: 4,
    },
    Question {
        prompt: "What is the maximum cluster size for Meridian and why?",
        expected: &["127", "7-bit", "node ID"],
        min_matches: 2,
    },
    Question {
        prompt: "What ports does the Meridian gossip protocol use?",
        expected: &["9471", "UDP", "9472", "TCP"],
        min_matches: 2,
    },
    Question {
        prompt: "What throughput did Meridian achieve on a 64-node cluster?",
        expected: &["47,000", "47000", "47ms"],
        min_matches: 1,
    },
    Question {
        prompt: "What is the Byzantine fault tolerance formula for Meridian?",
        expected: &["3f", "Byzantine", "2f", "crash"],
        min_matches: 2,
    },
    Question {
        prompt: "Name the three implementations of Meridian and their languages.",
        expected: &[
            "libmeridian",
            "C++",
            "meridian-rs",
            "Rust",
            "PyMeridian",
            "Python",
        ],
        min_matches: 4,
    },
    Question {
        prompt: "Which companies deployed Meridian in production?",
        expected: &["Cloudflare", "Akamai", "Fastly"],
        min_matches: 2,
    },
    Question {
        prompt: "What was Dr. Tanaka's prior work that influenced Meridian?",
        expected: &["Firefly", "gossip", "SOSP", "2017"],
        min_matches: 2,
    },
    Question {
        prompt: "What is the BLAME threshold and what happens when it's reached?",
        expected: &["f + 1", "blacklist", "10 epochs", "leader", "rotation"],
        min_matches: 2,
    },
];

/// Score an answer against expected keywords.
/// Returns (matches, total_expected, score_pct).
pub fn score_answer(answer: &str, question: &Question) -> (usize, usize, f64) {
    let answer_lower = answer.to_lowercase();
    let matches = question
        .expected
        .iter()
        .filter(|kw| answer_lower.contains(&kw.to_lowercase()))
        .count();
    let total = question.expected.len();
    let passed = matches >= question.min_matches;
    let score = if passed {
        100.0
    } else {
        (matches as f64 / question.min_matches as f64) * 100.0
    };
    (matches, total, score.min(100.0))
}

/// Session 1 prompt: read and memorize the document.
pub const SESSION1_PROMPT: &str = "\
Read the following document carefully. Memorize the key facts — names, numbers, dates, \
configurations, performance data, and technical details. Store everything important in memory \
so you can answer specific questions about it later.\n\n";

/// Prompt prefix for recall sessions (no document provided).
pub const RECALL_PREFIX: &str = "\
Answer the following question about the Meridian Protocol from memory. \
Be specific — include exact numbers, names, and technical details. \
If you stored information about this topic, recall it now.\n\n";
