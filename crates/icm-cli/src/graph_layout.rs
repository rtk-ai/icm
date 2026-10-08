//! Force-directed layout for the TUI's memory-relationship graph view.
//!
//! No external graph/physics crate: a basic spring embedder (inverse-square
//! repulsion between every node pair, spring attraction along edges,
//! velocity damping) is ~60 lines and plenty for the node counts a personal
//! memory store realistically has in a terminal view. O(n^2) repulsion is
//! the only real cost; see the perf note on [`compute_force_layout`].

use std::collections::HashMap;

/// Compute 2D positions for `ids` from a deterministic circular start.
///
/// `edges` are index pairs into `ids`. Returned coordinates are rescaled to
/// fit `[-1.0, 1.0]` on both axes (see [`normalize_to_unit_square`]): the
/// raw equilibrium extent grows with node count and shrinks with edge
/// density, so it can't be assumed. Starting from a fixed circular layout
/// (rather than random jitter) means re-entering the view without new data
/// reproduces the same picture.
///
/// Perf: this is O(n^2 * iterations) for the repulsion pass. Measured at
/// 200 iterations in a release build: 300 nodes ~50ms, 3300 nodes ~4s,
/// 12500 nodes ~50s. Callers must cap the node count (the TUI does, see
/// `GRAPH_MAX_NODES` in tui.rs) — it is a one-shot cost when the view is
/// opened, not something to run on a whole large store or every frame.
#[cfg(feature = "tui")]
pub fn compute_force_layout(
    ids: &[String],
    edges: &[(usize, usize)],
    iterations: usize,
) -> HashMap<String, (f64, f64)> {
    let n = ids.len();
    if n == 0 {
        return HashMap::new();
    }
    if n == 1 {
        let mut m = HashMap::with_capacity(1);
        m.insert(ids[0].clone(), (0.0, 0.0));
        return m;
    }

    let mut pos: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            (angle.cos(), angle.sin())
        })
        .collect();
    let mut vel = vec![(0.0_f64, 0.0_f64); n];

    const REPULSION: f64 = 0.02;
    const SPRING: f64 = 0.05;
    const SPRING_LEN: f64 = 0.6;
    const DAMPING: f64 = 0.85;
    const MIN_DIST: f64 = 0.01;
    // Per-node force cap: without it, two nodes that end up very close (a
    // dense graph — many memories hitting auto_link's max-links cap — makes
    // this common) produce a 1/dist^2 repulsion spike large enough that a
    // single step flings them across the layout, and the system re-injects
    // that much energy every step rather than settling — measured on a
    // 25-node/110-edge real graph, it never converges at all without this.
    const MAX_FORCE: f64 = 0.4;
    // Alpha cooling (same idea as d3-force's simulation.alpha()): scale
    // force strength by a temperature that decays every iteration on a
    // fixed schedule, independent of whether velocity happens to be
    // dropping. `iterations` is chosen so ALPHA_DECAY reaches a negligible
    // temperature well before the loop ends, guaranteeing a settled layout
    // instead of an arbitrary mid-chaos snapshot.
    const ALPHA_DECAY: f64 = 0.02;
    let mut alpha = 1.0_f64;

    for _ in 0..iterations {
        let mut force = vec![(0.0_f64, 0.0_f64); n];

        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let dist_sq = (dx * dx + dy * dy).max(MIN_DIST * MIN_DIST);
                let dist = dist_sq.sqrt();
                let f = (REPULSION / dist_sq).min(MAX_FORCE);
                let (fx, fy) = (f * dx / dist, f * dy / dist);
                force[i].0 += fx;
                force[i].1 += fy;
                force[j].0 -= fx;
                force[j].1 -= fy;
            }
        }

        for &(a, b) in edges {
            if a >= n || b >= n || a == b {
                continue;
            }
            let dx = pos[b].0 - pos[a].0;
            let dy = pos[b].1 - pos[a].1;
            let dist = (dx * dx + dy * dy).sqrt().max(MIN_DIST);
            let f = (SPRING * (dist - SPRING_LEN)).clamp(-MAX_FORCE, MAX_FORCE);
            let (fx, fy) = (f * dx / dist, f * dy / dist);
            force[a].0 += fx;
            force[a].1 += fy;
            force[b].0 -= fx;
            force[b].1 -= fy;
        }

        for i in 0..n {
            vel[i].0 = (vel[i].0 + force[i].0 * alpha) * DAMPING;
            vel[i].1 = (vel[i].1 + force[i].1 * alpha) * DAMPING;
            pos[i].0 += vel[i].0;
            pos[i].1 += vel[i].1;
        }
        alpha *= 1.0 - ALPHA_DECAY;
    }

    normalize_to_unit_square(&mut pos);
    ids.iter().cloned().zip(pos).collect()
}

/// Recenter `pos` on its bounding box and scale it uniformly (same factor
/// on both axes, so shapes aren't distorted) so the larger side spans
/// exactly `[-1.0, 1.0]`.
///
/// The simulation has no fixed scale: a sparse graph (most real stores —
/// the bulk of memories have no `related_ids` at all) is dominated by
/// repulsion and spreads to tens or hundreds of units, while a dense one
/// stays within a couple. The TUI draws a fixed window around the origin,
/// so without this a sparse graph rendered as one or two dots with every
/// other node off-screen.
#[cfg(feature = "tui")]
fn normalize_to_unit_square(pos: &mut [(f64, f64)]) {
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pos.iter() {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let half_extent = ((max_x - min_x) / 2.0).max((max_y - min_y) / 2.0);
    if !half_extent.is_finite() || half_extent <= 0.0 {
        return;
    }
    let (center_x, center_y) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    for p in pos.iter_mut() {
        p.0 = (p.0 - center_x) / half_extent;
        p.1 = (p.1 - center_y) / half_extent;
    }
}

/// 3D counterpart of [`compute_force_layout`], used by the web dashboard's
/// `/api/graph` endpoint (see `web.rs`).
///
/// The web graph view used to run this same physics in the browser, in
/// TypeScript, every time the page loaded. That's fine for the tens of
/// nodes a filtered topic view has, but a real store can have thousands of
/// memories: tested against one with 3286, the client-side version needed
/// ~5.4M pairwise force calculations *per animation frame* and never
/// visibly converged — the browser tab didn't crash, but the layout was
/// somewhere between "scattered off-screen" and "still computing" for as
/// long as the page stayed open. Computing it once here, server-side,
/// outside the 60fps/one-frame budget a browser is under, turns that into a
/// one-shot cost (same perf profile as the TUI's 2D layout above) and the
/// client just renders whatever positions it's given.
///
/// `clusters[i]` is a dense `0..k` topic index for node `i` (the caller
/// assigns these — this module has no notion of what a topic is). Plain
/// repulsion + edge springs alone settle a large, mostly-unlinked graph
/// (a real store has plenty of nodes with no `related_ids` at all) onto
/// the surface of a hollow sphere: every node repels every other node
/// equally, so the only stable configuration is "as far from everything
/// else as possible," which for N points in 3D is a shell. That shell
/// carries no information — verified by screenshotting a real 3301-node
/// store's unfiltered view, which rendered as an undifferentiated dot
/// cloud.
///
/// Two earlier attempts at fixing this didn't work, for the same
/// underlying reason. First, a per-iteration spring pulling each node
/// toward its topic's anchor, layered on top of unrestricted repulsion:
/// measurably no effect. Second, running the unrestricted physics
/// unchanged and then translating each topic's *centroid* onto an anchor
/// afterward: also no effect. The reason is the same in both cases —
/// repulsion is pairwise and antisymmetric (`force[i] += f; force[j] -=
/// f`), so summed over any group of nodes its net contribution to that
/// group's centroid is exactly zero; it only ever spreads a group's
/// members apart from *each other*, never moves the group as a whole.
/// With every node repelling every other node regardless of topic, a
/// topic's members end up scattered near-uniformly across the *entire*
/// shell (repulsion has no notion of "same topic," so there's nothing
/// pulling them together in the first place) — so neither an added pull
/// force (fighting a zero-sum opponent that still dominates locally) nor
/// a post-hoc centroid shift (recentering an already-diffuse point cloud
/// doesn't compact it) had anything real to work with.
///
/// The third attempt (this one) makes repulsion cluster-local (skip pairs
/// from different topics entirely — see the per-cluster `members` loop
/// below), so topics stop fighting each other into one shared
/// shell and each settles into its own locally-repelled blob. Cross-topic
/// edges (a real link between two different-topic memories) still apply
/// their spring regardless of cluster — that's real information, not
/// clustering noise. A per-iteration spring then pulls each node toward
/// its topic's anchor point, uncontested since nothing cancels it out
/// anymore.
///
/// That alone still wasn't enough — verified against real data (a
/// 3305-memory, 38-topic store): every topic's settled centroid landed at
/// roughly the *same* ~130-unit distance from the origin, regardless of
/// which anchor (spread ~490 units out) it was assigned. The anchor pull
/// shared `MAX_FORCE` with repulsion and springs, and a force capped at
/// 0.5 simply cannot move a node past a fixed ceiling within a bounded,
/// alpha-cooled iteration budget — see `CLUSTER_MAX_FORCE`'s comment for
/// the exact math. Every topic hit that same ceiling and stalled on the
/// surface of one shared-radius shell, which is indistinguishable from
/// the original undifferentiated sphere this fix exists to break up.
/// Giving the cluster pull its own, much higher force cap was the actual
/// missing piece.
#[cfg(feature = "web")]
pub fn compute_force_layout_3d(
    ids: &[String],
    edges: &[(usize, usize)],
    clusters: &[usize],
    iterations: usize,
) -> HashMap<String, (f64, f64, f64)> {
    let max_workers = std::thread::available_parallelism().map_or(1, |p| p.get());
    layout_3d(ids, edges, clusters, iterations, max_workers)
}

/// [`compute_force_layout_3d`] with the thread budget passed in rather than
/// read from the machine, so a test can check the result is the same
/// whatever the split.
#[cfg(feature = "web")]
fn layout_3d(
    ids: &[String],
    edges: &[(usize, usize)],
    clusters: &[usize],
    iterations: usize,
    max_workers: usize,
) -> HashMap<String, (f64, f64, f64)> {
    let n = ids.len();
    if n == 0 {
        return HashMap::new();
    }
    if n == 1 {
        let mut m = HashMap::with_capacity(1);
        m.insert(ids[0].clone(), (0.0, 0.0, 0.0));
        return m;
    }

    // Fibonacci sphere start (matches the web client's previous JS
    // implementation): evenly distributed in 3D from the first iteration,
    // unlike a naive lat/long grid which clumps points at the poles.
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let mut pos: Vec<(f64, f64, f64)> = (0..n)
        .map(|i| {
            let t = if n > 1 {
                i as f64 / (n - 1) as f64
            } else {
                0.0
            };
            let y = 1.0 - 2.0 * t;
            let radius_at_y = (1.0 - y * y).max(0.0).sqrt();
            let theta = golden_angle * i as f64;
            (theta.cos() * radius_at_y, y, theta.sin() * radius_at_y)
        })
        .collect();
    let mut vel = vec![(0.0_f64, 0.0_f64, 0.0_f64); n];

    const REPULSION: f64 = 0.03;
    const SPRING: f64 = 0.05;
    const SPRING_LEN: f64 = 0.9;
    const DAMPING: f64 = 0.85;
    const MIN_DIST: f64 = 0.02;
    const MAX_FORCE: f64 = 0.5;
    const ALPHA_DECAY: f64 = 0.02;
    // Strength of the pull toward a node's topic anchor. Comparable to
    // SPRING rather than a token nudge: with repulsion now cluster-local
    // (below), this is the *only* force placing a topic as a whole, so it
    // needs to be assertive enough to actually move a topic's blob across
    // real distances within the iteration budget, not just perturb it.
    const CLUSTER_PULL: f64 = 0.05;
    // Cluster pull's own force cap — deliberately NOT the shared
    // MAX_FORCE. A force capped at F, under this DAMPING/ALPHA_DECAY
    // schedule over `iterations` steps, can physically never move a node
    // more than a fixed ceiling regardless of how far its target is
    // (leaky-integrator math: velocity settles to D*F/(1-D) per step
    // while alpha~1, decaying as alpha does) — at MAX_FORCE=0.5 that
    // ceiling is ~139 units for a 200-iteration budget. Verified this was
    // the actual bug behind two failed attempts at this: with anchors
    // spread ~490 units out (38 real topics), every topic's settled
    // centroid landed at ~120-140 units from origin regardless of which
    // anchor it was assigned — capped at the same reachable-distance
    // ceiling, so every topic ended up on the surface of one shared-radius
    // shell at different angles, which *looks* like the exact same
    // undifferentiated sphere this whole fix was meant to break up. Giving
    // the cluster pull its own much higher cap raises that ceiling well
    // past any anchor distance this module will realistically produce.
    const CLUSTER_MAX_FORCE: f64 = 4.0;
    // Anchor-to-anchor spacing. Deliberately large: a dominant topic (a
    // real store's largest topic can hold a majority of all memories) is
    // going to spread itself, via its own internal repulsion, over a
    // radius not much smaller than the *whole* graph would have used
    // unclustered — small anchor spacing would let that one topic's own
    // spread swallow every other topic's anchor point. Scaled by
    // sqrt(cluster count) so the constant means roughly the same thing
    // whether there are 5 topics or 80.
    const CLUSTER_SPACING: f64 = 80.0;
    let mut alpha = 1.0_f64;

    let cluster_count = clusters.iter().copied().max().map_or(1, |m| m + 1);
    let cluster_of = |i: usize| clusters.get(i).copied().unwrap_or(0).min(cluster_count - 1);
    let cluster_anchors: Vec<(f64, f64, f64)> = if cluster_count <= 1 {
        vec![(0.0, 0.0, 0.0)]
    } else {
        let radius = CLUSTER_SPACING * (cluster_count as f64).sqrt();
        (0..cluster_count)
            .map(|i| {
                let t = i as f64 / (cluster_count - 1) as f64;
                let y = 1.0 - 2.0 * t;
                let radius_at_y = (1.0 - y * y).max(0.0).sqrt();
                let theta = golden_angle * i as f64;
                (
                    theta.cos() * radius_at_y * radius,
                    y * radius,
                    theta.sin() * radius_at_y * radius,
                )
            })
            .collect()
    };

    // Node indices grouped by cluster, each group in ascending index order.
    // Repulsion only acts within a topic (see `repulsion_on`), so each
    // node only ever needs its own cluster's members — not a scan of all
    // n^2 pairs that skips the cross-topic ones, which on a 12.5k-memory,
    // 117-topic store was ~78M pair checks per iteration for ~8M that
    // actually apply.
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); cluster_count];
    for i in 0..n {
        members[cluster_of(i)].push(i);
    }

    // Net repulsion on node `i` from every other member of its own cluster.
    //
    // Repulsion only within a topic: unrestricted repulsion is
    // pairwise-antisymmetric, so summed over a whole cluster it nets to
    // zero on that cluster's centroid — it can only spread a cluster's own
    // members apart, never move the cluster as a whole. Restricting it is
    // what lets the anchor pull below actually place each topic, instead
    // of fighting an opponent it can't win against once every node repels
    // every other node.
    //
    // Computed per node (reading positions, writing nothing shared) rather
    // than per pair with `force[i] += f; force[j] -= f`: that visits each
    // pair twice, but it is what lets rows be spread across threads below.
    fn repulsion_on(i: usize, group: &[usize], pos: &[(f64, f64, f64)]) -> (f64, f64, f64) {
        let mut sum = (0.0_f64, 0.0_f64, 0.0_f64);
        for &j in group {
            if j == i {
                continue;
            }
            let dx = pos[i].0 - pos[j].0;
            let dy = pos[i].1 - pos[j].1;
            let dz = pos[i].2 - pos[j].2;
            let dist_sq = (dx * dx + dy * dy + dz * dz).max(MIN_DIST * MIN_DIST);
            let dist = dist_sq.sqrt();
            let scale = (REPULSION / dist_sq).min(MAX_FORCE) / dist;
            sum.0 += scale * dx;
            sum.1 += scale * dy;
            sum.2 += scale * dz;
        }
        sum
    }

    // Repulsion is the whole cost of this function: O(sum of cluster
    // size^2) per iteration, and one dominant topic (thousands of
    // memories is real) makes that tens of millions of pairs. Measured on
    // a real 12.5k-memory store, single-threaded, it took ~19s per
    // request. Rows are independent, so above a size where it pays for
    // the per-iteration thread spawns, split them across cores. The
    // result does not depend on the split: each row sums the same terms
    // in the same order whichever thread runs it.
    const PARALLEL_MIN_COST: usize = 1_000_000;
    let total_cost: usize = members.iter().map(|g| g.len() * g.len()).sum();
    let workers = if total_cost < PARALLEL_MIN_COST {
        1
    } else {
        max_workers.max(1)
    };
    // Nodes in cluster order, cut into at most `workers` runs of roughly
    // equal cost (a node's row costs its cluster's size), so one thread
    // doesn't end up holding the whole dominant topic.
    let cost_per_chunk = total_cost.div_ceil(workers).max(1);
    let mut chunks: Vec<Vec<usize>> = vec![Vec::new()];
    let mut chunk_cost = 0;
    for group in &members {
        for &i in group {
            if chunk_cost >= cost_per_chunk && chunks.len() < workers {
                chunks.push(Vec::new());
                chunk_cost = 0;
            }
            let last = chunks.len() - 1;
            chunks[last].push(i);
            chunk_cost += group.len();
        }
    }

    for _ in 0..iterations {
        let mut force = vec![(0.0_f64, 0.0_f64, 0.0_f64); n];

        let rows = |chunk: &[usize]| -> Vec<(f64, f64, f64)> {
            chunk
                .iter()
                .map(|&i| repulsion_on(i, &members[cluster_of(i)], &pos))
                .collect()
        };
        if chunks.len() == 1 {
            for (&i, f) in chunks[0].iter().zip(rows(&chunks[0])) {
                force[i] = f;
            }
        } else {
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunks
                    .iter()
                    .map(|chunk| scope.spawn(|| rows(chunk)))
                    .collect();
                for (chunk, handle) in chunks.iter().zip(handles) {
                    match handle.join() {
                        Ok(computed) => {
                            for (&i, f) in chunk.iter().zip(computed) {
                                force[i] = f;
                            }
                        }
                        Err(panic) => std::panic::resume_unwind(panic),
                    }
                }
            });
        }

        for &(a, b) in edges {
            if a >= n || b >= n || a == b {
                continue;
            }
            let dx = pos[b].0 - pos[a].0;
            let dy = pos[b].1 - pos[a].1;
            let dz = pos[b].2 - pos[a].2;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(MIN_DIST);
            let f = (SPRING * (dist - SPRING_LEN)).clamp(-MAX_FORCE, MAX_FORCE);
            let (fx, fy, fz) = (f * dx / dist, f * dy / dist, f * dz / dist);
            force[a].0 += fx;
            force[a].1 += fy;
            force[a].2 += fz;
            force[b].0 -= fx;
            force[b].1 -= fy;
            force[b].2 -= fz;
        }

        for i in 0..n {
            let anchor = cluster_anchors[cluster_of(i)];
            let dx = anchor.0 - pos[i].0;
            let dy = anchor.1 - pos[i].1;
            let dz = anchor.2 - pos[i].2;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(MIN_DIST);
            let f = (CLUSTER_PULL * dist).min(CLUSTER_MAX_FORCE);
            force[i].0 += f * dx / dist;
            force[i].1 += f * dy / dist;
            force[i].2 += f * dz / dist;
        }

        for i in 0..n {
            vel[i].0 = (vel[i].0 + force[i].0 * alpha) * DAMPING;
            vel[i].1 = (vel[i].1 + force[i].1 * alpha) * DAMPING;
            vel[i].2 = (vel[i].2 + force[i].2 * alpha) * DAMPING;
            pos[i].0 += vel[i].0;
            pos[i].1 += vel[i].1;
            pos[i].2 += vel[i].2;
        }
        alpha *= 1.0 - ALPHA_DECAY;
    }

    ids.iter().cloned().zip(pos).collect()
}

#[cfg(all(test, feature = "tui"))]
mod tests_2d {
    use super::*;

    #[test]
    fn empty_input_returns_empty_map() {
        assert!(compute_force_layout(&[], &[], 50).is_empty());
    }

    #[test]
    fn single_node_sits_at_origin() {
        let ids = vec!["a".to_string()];
        let pos = compute_force_layout(&ids, &[], 50);
        assert_eq!(pos.get("a"), Some(&(0.0, 0.0)));
    }

    #[test]
    fn connected_nodes_end_up_closer_than_unconnected_ones() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        // a-b is an edge; c is isolated.
        let pos = compute_force_layout(&ids, &[(0, 1)], 300);
        let dist =
            |p: (f64, f64), q: (f64, f64)| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
        let d_ab = dist(pos["a"], pos["b"]);
        let d_ac = dist(pos["a"], pos["c"]);
        assert!(
            d_ab < d_ac,
            "expected linked nodes closer together: d_ab={d_ab} d_ac={d_ac}"
        );
    }

    #[test]
    fn is_deterministic_across_runs() {
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let edges = [(0, 1), (1, 2), (2, 3)];
        let p1 = compute_force_layout(&ids, &edges, 100);
        let p2 = compute_force_layout(&ids, &edges, 100);
        assert_eq!(p1, p2);
    }

    #[test]
    fn out_of_range_edge_indices_are_ignored_not_panicking() {
        let ids: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        // Should not panic despite the out-of-bounds index.
        let pos = compute_force_layout(&ids, &[(0, 99)], 10);
        assert_eq!(pos.len(), 2);
    }

    /// Regression: the raw simulation has no fixed scale. A sparse graph
    /// (few edges, like most real stores) spread to tens of units, so the
    /// TUI's fixed window around the origin showed 0-2 nodes out of
    /// hundreds. Whatever the density, the result must fill — and stay
    /// inside — the unit square the TUI draws.
    #[test]
    fn sparse_and_dense_graphs_both_fill_the_unit_square() {
        let ids: Vec<String> = (0..300).map(|i| i.to_string()).collect();
        let sparse: Vec<(usize, usize)> = (0..30).map(|i| (i, (i * 7 + 3) % 300)).collect();
        let dense: Vec<(usize, usize)> = (0..300)
            .flat_map(|i| (1..6).map(move |k| (i, (i + k) % 300)))
            .collect();
        for (name, edges) in [("no edges", vec![]), ("sparse", sparse), ("dense", dense)] {
            let pos = compute_force_layout(&ids, &edges, 200);
            let extent = pos
                .values()
                .map(|p| p.0.abs().max(p.1.abs()))
                .fold(0.0, f64::max);
            assert!(
                (extent - 1.0).abs() < 1e-9,
                "{name}: expected the layout to span exactly [-1, 1], got max |coord| {extent}"
            );
        }
    }
}

#[cfg(all(test, feature = "web"))]
mod tests_3d {
    use super::*;

    #[test]
    fn compute_force_layout_3d_empty_input_returns_empty_map() {
        assert!(compute_force_layout_3d(&[], &[], &[], 50).is_empty());
    }

    #[test]
    fn compute_force_layout_3d_single_node_sits_at_origin() {
        let ids = vec!["a".to_string()];
        let pos = compute_force_layout_3d(&ids, &[], &[0], 50);
        assert_eq!(pos.get("a"), Some(&(0.0, 0.0, 0.0)));
    }

    #[test]
    fn compute_force_layout_3d_connected_nodes_end_up_closer_than_unconnected_ones() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let pos = compute_force_layout_3d(&ids, &[(0, 1)], &[0, 0, 0], 300);
        let dist = |p: (f64, f64, f64), q: (f64, f64, f64)| {
            ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
        };
        let d_ab = dist(pos["a"], pos["b"]);
        let d_ac = dist(pos["a"], pos["c"]);
        assert!(
            d_ab < d_ac,
            "expected linked nodes closer together: d_ab={d_ab} d_ac={d_ac}"
        );
    }

    #[test]
    fn compute_force_layout_3d_is_deterministic_across_runs() {
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let edges = [(0, 1), (1, 2), (2, 3)];
        let clusters = [0, 0, 1, 1];
        let p1 = compute_force_layout_3d(&ids, &edges, &clusters, 100);
        let p2 = compute_force_layout_3d(&ids, &edges, &clusters, 100);
        assert_eq!(p1, p2);
    }

    #[test]
    fn compute_force_layout_3d_out_of_range_edge_indices_are_ignored_not_panicking() {
        let ids: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let pos = compute_force_layout_3d(&ids, &[(0, 99)], &[0, 0], 10);
        assert_eq!(pos.len(), 2);
    }

    #[test]
    fn compute_force_layout_3d_actually_uses_all_three_dimensions() {
        // A layout that only ever moved nodes in the XY plane would still
        // pass every test above — this is the one that would actually catch
        // a copy-paste of the 2D version that forgot to touch Z.
        let ids: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let clusters = vec![0usize; 30];
        let pos = compute_force_layout_3d(&ids, &[], &clusters, 200);
        let z_spread = pos
            .values()
            .map(|p| p.2)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), z| {
                (lo.min(z), hi.max(z))
            });
        assert!(
            z_spread.1 - z_spread.0 > 0.5,
            "expected real spread on the Z axis, got range {z_spread:?}"
        );
    }

    #[test]
    fn compute_force_layout_3d_same_cluster_nodes_end_up_closer_than_different_clusters() {
        // No edges at all — the only thing pulling these apart from a
        // uniform shell is the topic-cluster force. 8 same-topic nodes vs.
        // 8 other-topic nodes, no links between them: same-cluster pairs
        // must land closer together than cross-cluster pairs, or the
        // "topic clouds" effect isn't real.
        let ids: Vec<String> = (0..16).map(|i| format!("n{i}")).collect();
        let clusters: Vec<usize> = (0..16).map(|i| if i < 8 { 0 } else { 1 }).collect();
        let pos = compute_force_layout_3d(&ids, &[], &clusters, 300);
        let dist = |p: (f64, f64, f64), q: (f64, f64, f64)| {
            ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
        };
        let same_cluster = dist(pos["n0"], pos["n1"]);
        let cross_cluster = dist(pos["n0"], pos["n8"]);
        assert!(
            same_cluster < cross_cluster,
            "expected same-topic nodes closer together: same={same_cluster} cross={cross_cluster}"
        );
    }

    /// Cluster indices arrive interleaved in real data (memories are listed
    /// by weight, not grouped by topic), so the per-cluster grouping must
    /// not assume contiguous runs: nodes sharing a topic still end up
    /// together, and nodes in different topics on separate anchors.
    #[test]
    fn compute_force_layout_3d_groups_interleaved_cluster_indices() {
        let ids: Vec<String> = (0..6).map(|i| format!("n{i}")).collect();
        let pos = compute_force_layout_3d(&ids, &[], &[0, 1, 0, 1, 0, 1], 300);
        let dist = |a: &str, b: &str| {
            let (p, q) = (pos[a], pos[b]);
            ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt()
        };
        for (same, other) in [("n2", "n1"), ("n4", "n3"), ("n2", "n5")] {
            assert!(
                dist("n0", same) < dist("n0", other),
                "n0 must be closer to same-topic {same} than to other-topic {other}"
            );
        }
        // Same-topic nodes repel each other; they must not collapse onto
        // one point.
        assert!(dist("n0", "n2") > 0.05);
    }

    /// Large clusters are laid out across threads. The split must not
    /// change the result: a graph big enough to take the threaded path
    /// (one dominant topic plus small ones, some cross-topic links) has to
    /// come out bit-for-bit identical on 1, 2 and 7 workers.
    #[test]
    fn layout_3d_result_does_not_depend_on_the_worker_count() {
        let n = 1300;
        let ids: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        let clusters: Vec<usize> = (0..n)
            .map(|i| if i % 13 == 0 { 1 + i % 3 } else { 0 })
            .collect();
        let edges: Vec<(usize, usize)> = (0..200).map(|i| (i, (i * 31 + 7) % n)).collect();

        let single = layout_3d(&ids, &edges, &clusters, 2, 1);
        assert_eq!(single.len(), n);
        assert!(
            single
                .values()
                .all(|p| p.0.is_finite() && p.1.is_finite() && p.2.is_finite())
        );
        for workers in [2, 7] {
            assert_eq!(
                layout_3d(&ids, &edges, &clusters, 2, workers),
                single,
                "layout differs with {workers} workers"
            );
        }
    }
}
