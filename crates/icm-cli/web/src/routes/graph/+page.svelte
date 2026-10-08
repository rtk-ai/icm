<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import * as THREE from 'three';
	import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
	import { CSS2DRenderer, CSS2DObject } from 'three/addons/renderers/CSS2DRenderer.js';
	import { LineSegments2 } from 'three/addons/lines/LineSegments2.js';
	import { LineSegmentsGeometry } from 'three/addons/lines/LineSegmentsGeometry.js';
	import { LineMaterial } from 'three/addons/lines/LineMaterial.js';
	import { api } from '$lib/api';
	import type { GraphNode, GraphResponse, TopicEntry } from '$lib/types';

	let containerEl: HTMLDivElement = $state()!;
	let graph: GraphResponse = $state({ nodes: [], edges: [] });
	let topics: TopicEntry[] = $state([]);
	let selectedTopic = $state('');
	let loading = $state(true);
	let loadError = $state('');
	let selected: GraphNode | null = $state(null);
	// Cross-topic links are common — roughly 2/3 of all links in a real
	// store — and since clustering now spreads topics far apart (that's
	// the point), each one renders as a long line crossing empty space.
	// Fine for the tens of edges a filtered view has; hundreds of them
	// turn an unfiltered view into a dense crossing web that hides the
	// cluster structure the layout just worked to produce. `load()` sets
	// a size-based default each time the topic changes; the checkbox
	// below lets it be overridden either way.
	let showLinks = $state(true);
	// The selected node's neighbors, with the real similarity score that
	// produced the edge (not just "linked: yes/no") — sorted strongest
	// first, since that's usually what you actually want to see.
	let linkedNodes = $derived.by(() => {
		if (!selected) return [];
		const byId = new Map(graph.nodes.map(n => [n.id, n]));
		return graph.edges
			.filter(e => e.source === selected!.id || e.target === selected!.id)
			.map(e => {
				const otherId = e.source === selected!.id ? e.target : e.source;
				return { node: byId.get(otherId), similarity: e.similarity };
			})
			.filter((l): l is { node: GraphNode; similarity: number } => l.node !== undefined)
			.sort((a, b) => b.similarity - a.similarity);
	});

	// This app has zero runtime JS dependencies by design, except three.js
	// here: a 3D force-directed graph is exactly the case where a hand-rolled
	// renderer stops paying for itself — camera orbit/zoom/pan, perspective
	// projection, and depth sorting are all things three.js gets right that
	// a raw-WebGL2 version (this page's first draft) had to fake in 2D.

	// --- Simulation state (world space; positions are pre-computed
	// server-side, see buildSim below — nothing here runs physics) ---
	interface SimNode {
		id: string;
		x: number;
		y: number;
		z: number;
		color: THREE.Color;
		size: number;
	}
	let simNodes: SimNode[] = [];
	// [sourceIdx, targetIdx, similarity] — indices into simNodes.
	let simEdges: [number, number, number][] = [];
	let idToIndex = new Map<string, number>();

	// importance -> color/base size. Matches this app's own existing
	// convention from routes/memories/+page.svelte's importanceColor()
	// (red/orange/blue/gray). Size scales with criticality first, memory
	// weight second (a critical memory should read as visually dominant
	// even before you know its weight).
	function importanceColor(imp: string): THREE.Color {
		switch (imp) {
			case 'critical':
				return new THREE.Color(0.96, 0.32, 0.32);
			case 'high':
				return new THREE.Color(0.98, 0.58, 0.24);
			case 'medium':
				return new THREE.Color(0.38, 0.65, 0.98);
			default:
				return new THREE.Color(0.61, 0.64, 0.69);
		}
	}
	function importanceBaseSize(imp: string): number {
		switch (imp) {
			case 'critical':
				return 46;
			case 'high':
				return 34;
			case 'medium':
				return 24;
			default:
				return 16;
		}
	}

	// Positions come pre-computed from the server (graph_layout::compute_force_layout_3d
	// in web.rs) rather than from physics run in the browser — see that
	// function's doc comment for why: client-side JS physics on a real
	// multi-thousand-memory store never converged and effectively hung the
	// page. `simNodes` is therefore just a typed view over `g.nodes` for the
	// renderer to read; nothing here moves anything.
	function buildSim(g: GraphResponse) {
		idToIndex = new Map(g.nodes.map((n, i) => [n.id, i]));
		simNodes = g.nodes.map(node => ({
			id: node.id,
			x: node.x,
			y: node.y,
			z: node.z,
			color: importanceColor(node.importance),
			size: importanceBaseSize(node.importance) * (0.7 + Math.min(node.weight, 1) * 0.5),
		}));
		simEdges = g.edges
			.map(
				e =>
					[idToIndex.get(e.source), idToIndex.get(e.target), e.similarity] as [
						number | undefined,
						number | undefined,
						number,
					],
			)
			.filter((e): e is [number, number, number] => e[0] !== undefined && e[1] !== undefined);
	}


	// --- three.js scene ---
	let scene: THREE.Scene | null = null;
	let camera: THREE.PerspectiveCamera | null = null;
	let renderer: THREE.WebGLRenderer | null = null;
	let labelRenderer: CSS2DRenderer | null = null;
	let controls: OrbitControls | null = null;
	let raycaster: THREE.Raycaster | null = null;
	let pointsObj: THREE.Points | null = null;
	let nodeGeometry: THREE.BufferGeometry | null = null;
	let labelObjects: CSS2DObject[] = [];
	let pointerDownPos: { x: number; y: number } | null = null;
	// THREE.Clock is deprecated in favor of Timer as of three r180-ish —
	// using it spammed a console warning on every Graph page load.
	let clock = new THREE.Timer();

	// Edge "size" = real line width (not just opacity — WebGL's native
	// gl.LINES has no reliable width across GL drivers, but three.js's fat
	// line addon (LineSegments2/LineMaterial) draws lines as camera-facing
	// quads instead, so a real pixel width works everywhere). LineMaterial
	// has one width per material instance, not per segment, so edges are
	// bucketed into similarity tiers, each its own LineSegments2 — three
	// tiers reads clearly without needing a continuous per-edge width.
	// auto_link.rs's own link threshold is 0.75, so tiers are anchored
	// there rather than at 0, or nearly every real edge would land in the
	// same bucket.
	// The 0-0.75 bucket isn't really "weak similarity" — auto_link.rs never
	// creates a real link below its own 0.75 threshold, so an edge scoring
	// there means the backend couldn't compute a similarity at all (one or
	// both memories have no stored embedding — a real, common case: ~60% of
	// memories in a live store checked during this feature's development
	// had none, likely stored with `--no-embeddings` or before an embedder
	// was configured). Rendered distinctly (dashed-looking via low opacity)
	// rather than silently dropped, which is what happened before this
	// bucket existed — a topic made entirely of no-embedding memories
	// rendered as nodes with literally no visible edges.
	const EDGE_TIERS = [
		// 0.15 opacity here (this file's first version) read as "no edges at
		// all" on a topic-filtered view where most/all real similarity
		// scores are unknown (missing embeddings — a common real-data case,
		// see the comment above) — bumped so an unscored link still reads
		// as a real connection instead of basically invisible.
		{ min: 0, max: 0.75, width: 1, color: 0x475569, opacity: 0.35 },
		{ min: 0.75, max: 0.85, width: 1.4, color: 0x64748b, opacity: 0.5 },
		{ min: 0.85, max: 0.93, width: 2.2, color: 0x7c8ba1, opacity: 0.65 },
		{ min: 0.93, max: 1.01, width: 3.6, color: 0x94a3b8, opacity: 0.85 },
	];
	let edgeTierObjs: LineSegments2[] = [];
	let topicSphereObjs: THREE.Mesh[] = [];

	// "Information transiting" — small pulses travel from source to target
	// along each edge, looping continuously. Stronger links (higher
	// similarity) pulse faster and brighter, so the strongest relationships
	// in the graph visibly carry more "traffic" — reuses the same
	// similarity data edge width does, just as motion instead of thickness.
	const PULSES_PER_EDGE = 2;
	let pulseObj: THREE.Points | null = null;
	let pulseGeometry: THREE.BufferGeometry | null = null;

	// Point size falls off with distance from the camera (perspective depth
	// cue) via gl_PointSize's 1/-z term — the same trick three.js's own
	// point-cloud examples use, since PointsMaterial's built-in sizeAttenuation
	// doesn't support a per-vertex size attribute on top of it. Clamped to a
	// floor: this formula was tuned when the whole layout fit in a few
	// hundred units, so fitCameraToNodes' camera distance stayed small — now
	// that topic clusters spread over ~500 units (see graph_layout.rs's
	// CLUSTER_SPACING), the same camera has to sit far enough back that
	// 1/-z alone shrinks every point under a pixel, i.e. invisible. The
	// ceiling matters too, just less urgently: without it, zooming in close
	// inflates points into giant blobs.
	const NODE_VS = `
		attribute float a_size;
		attribute vec3 a_color;
		varying vec3 v_color;
		void main() {
			vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
			gl_PointSize = clamp(a_size * (3.0 / -mvPosition.z), 3.0, 60.0);
			gl_Position = projectionMatrix * mvPosition;
			v_color = a_color;
		}
	`;
	const NODE_FS = `
		precision mediump float;
		varying vec3 v_color;
		void main() {
			vec2 c = gl_PointCoord - vec2(0.5);
			float d = length(c);
			if (d > 0.5) discard;
			float glow = smoothstep(0.5, 0.15, d);
			gl_FragColor = vec4(v_color, glow);
		}
	`;
	// Each pulse walks its edge over [0,1) on a loop; a_speed (derived from
	// similarity) controls how fast, so stronger links visibly carry more
	// "traffic". Position is computed on the GPU (mix of the edge's two
	// endpoints) rather than updated from JS every frame, which would mean
	// touching every pulse's buffer entry 60 times a second.
	const PULSE_VS = `
		attribute vec3 a_start;
		attribute vec3 a_end;
		attribute float a_speed;
		attribute float a_offset;
		uniform float u_time;
		varying float v_glow;
		void main() {
			float t = fract(u_time * a_speed + a_offset);
			vec3 pos = mix(a_start, a_end, t);
			vec4 mvPosition = modelViewMatrix * vec4(pos, 1.0);
			gl_PointSize = clamp(5.0 * (3.0 / -mvPosition.z), 2.0, 20.0);
			gl_Position = projectionMatrix * mvPosition;
			// Fade in/out at each end of the run instead of popping when it
			// loops back to the start.
			v_glow = smoothstep(0.0, 0.08, t) * (1.0 - smoothstep(0.92, 1.0, t));
		}
	`;
	const PULSE_FS = `
		precision mediump float;
		varying float v_glow;
		void main() {
			vec2 c = gl_PointCoord - vec2(0.5);
			float d = length(c);
			if (d > 0.5) discard;
			float glow = smoothstep(0.5, 0.1, d);
			gl_FragColor = vec4(0.65, 0.85, 1.0, glow * v_glow * 0.9);
		}
	`;

	function edgePositionsForTier(tier: (typeof EDGE_TIERS)[number]): number[] {
		const positions: number[] = [];
		for (const [a, b, sim] of simEdges) {
			if (sim < tier.min || sim >= tier.max) continue;
			positions.push(simNodes[a].x, simNodes[a].y, simNodes[a].z, simNodes[b].x, simNodes[b].y, simNodes[b].z);
		}
		return positions;
	}

	function buildScene() {
		if (!scene || !renderer) return;
		if (pointsObj) scene.remove(pointsObj);
		for (const obj of edgeTierObjs) scene.remove(obj);
		edgeTierObjs = [];
		for (const obj of topicSphereObjs) {
			scene.remove(obj);
			obj.geometry.dispose();
			(obj.material as THREE.Material).dispose();
		}
		topicSphereObjs = [];
		if (pulseObj) scene.remove(pulseObj);
		for (const l of labelObjects) l.element.remove();
		labelObjects = [];

		const n = simNodes.length;
		const positions = new Float32Array(n * 3);
		const colors = new Float32Array(n * 3);
		const sizes = new Float32Array(n);
		for (let i = 0; i < n; i++) {
			positions[i * 3] = simNodes[i].x;
			positions[i * 3 + 1] = simNodes[i].y;
			positions[i * 3 + 2] = simNodes[i].z;
			colors[i * 3] = simNodes[i].color.r;
			colors[i * 3 + 1] = simNodes[i].color.g;
			colors[i * 3 + 2] = simNodes[i].color.b;
			sizes[i] = simNodes[i].size;
		}
		nodeGeometry = new THREE.BufferGeometry();
		nodeGeometry.setAttribute('position', new THREE.BufferAttribute(positions, 3));
		nodeGeometry.setAttribute('a_color', new THREE.BufferAttribute(colors, 3));
		nodeGeometry.setAttribute('a_size', new THREE.BufferAttribute(sizes, 1));
		const nodeMaterial = new THREE.ShaderMaterial({
			vertexShader: NODE_VS,
			fragmentShader: NODE_FS,
			transparent: true,
			depthWrite: false,
		});
		pointsObj = new THREE.Points(nodeGeometry, nodeMaterial);
		scene.add(pointsObj);

		if (showLinks) {
			for (const tier of EDGE_TIERS) {
				const positions = edgePositionsForTier(tier);
				if (positions.length === 0) continue;
				const geometry = new LineSegmentsGeometry();
				geometry.setPositions(positions);
				const material = new LineMaterial({
					color: tier.color,
					linewidth: tier.width,
					transparent: true,
					opacity: tier.opacity,
					depthWrite: false,
				});
				material.resolution.set(renderer.domElement.clientWidth, renderer.domElement.clientHeight);
				const obj = new LineSegments2(geometry, material);
				scene.add(obj);
				edgeTierObjs.push(obj);
			}
		}

		// Traveling pulses: a_start/a_end per pulse (updated whenever nodes
		// move), a_speed scaled by similarity (stronger link = faster pulse
		// = visibly more "traffic"), a_offset randomized so pulses on the
		// same edge don't move in lockstep. Skipped along with the edge
		// lines themselves when links are hidden — a pulse with no visible
		// line under it just reads as a stray moving dot, not "traffic".
		const pulseCount = showLinks ? simEdges.length * PULSES_PER_EDGE : 0;
		const pStart = new Float32Array(pulseCount * 3);
		const pEnd = new Float32Array(pulseCount * 3);
		const pSpeed = new Float32Array(pulseCount);
		const pOffset = new Float32Array(pulseCount);
		if (showLinks) {
			for (let i = 0; i < simEdges.length; i++) {
				const [a, b, sim] = simEdges[i];
				for (let k = 0; k < PULSES_PER_EDGE; k++) {
					const idx = i * PULSES_PER_EDGE + k;
					pStart[idx * 3] = simNodes[a].x;
					pStart[idx * 3 + 1] = simNodes[a].y;
					pStart[idx * 3 + 2] = simNodes[a].z;
					pEnd[idx * 3] = simNodes[b].x;
					pEnd[idx * 3 + 1] = simNodes[b].y;
					pEnd[idx * 3 + 2] = simNodes[b].z;
					// 0.75-1.0 similarity -> a full loop every ~10s down to ~3s.
					pSpeed[idx] = 0.1 + Math.max(sim - 0.75, 0) * 1.1;
					pOffset[idx] = (k + Math.random()) / PULSES_PER_EDGE;
				}
			}
		}
		pulseGeometry = new THREE.BufferGeometry();
		// A `position` attribute isn't used by PULSE_VS (it computes gl_Position
		// from a_start/a_end instead) but geometry.computeBoundingSphere() —
		// which THREE.Points.raycast/frustum-culling relies on — needs one to
		// exist, or the whole object silently renders nothing.
		pulseGeometry.setAttribute('position', new THREE.BufferAttribute(pStart.slice(), 3));
		pulseGeometry.setAttribute('a_start', new THREE.BufferAttribute(pStart, 3));
		pulseGeometry.setAttribute('a_end', new THREE.BufferAttribute(pEnd, 3));
		pulseGeometry.setAttribute('a_speed', new THREE.BufferAttribute(pSpeed, 1));
		pulseGeometry.setAttribute('a_offset', new THREE.BufferAttribute(pOffset, 1));
		const pulseMaterial = new THREE.ShaderMaterial({
			vertexShader: PULSE_VS,
			fragmentShader: PULSE_FS,
			uniforms: { u_time: { value: 0 } },
			transparent: true,
			depthWrite: false,
		});
		pulseObj = new THREE.Points(pulseGeometry, pulseMaterial);
		// Position is animated per-vertex on the GPU (see PULSE_VS), so the
		// `position` attribute above is a bounding-sphere stand-in only —
		// three.js's static frustum culling would otherwise clip pulses that
		// have walked away from their (fixed) `position` value.
		pulseObj.frustumCulled = false;
		scene.add(pulseObj);

		// Always-on labels (not just on click). Two different jobs depending
		// on the view:
		//
		// Unfiltered ("All topics"): the question a reader actually has is
		// "which blob is which topic" — a per-node summary can't answer
		// that (it names one memory, not the cluster it sits in), and
		// picking one per topic still means up to ~40 competing text boxes
		// scattered near each other in screen space once several clusters
		// project close together. Labeling the *cluster* itself — its
		// topic name + memory count, once, at the cluster's own centroid —
		// answers the actual question with exactly one label per cluster,
		// no per-node redundancy to overlap. This is the "topic clouds"
		// idea from earlier: name the cloud, not a random point inside it.
		//
		// Filtered to one topic: every node already shares that topic, so
		// a topic label would be blank information repeated on every node
		// — the summary is what actually distinguishes one memory from the
		// next here, so keep per-node summary labels, capped and picked by
		// importance like before.
		if (!selectedTopic) {
			const byTopic = new Map<string, number[]>();
			for (let i = 0; i < n; i++) {
				const topic = graph.nodes[i].topic;
				const list = byTopic.get(topic);
				if (list) list.push(i);
				else byTopic.set(topic, [i]);
			}

			// Pass 1: each topic's centroid and an unconstrained radius from
			// its own spread. 90th-percentile distance rather than the max:
			// one stray outlier (auto_link's cosine-similarity linking can
			// put a single node far from its topic's own bulk) would
			// otherwise balloon the whole sphere around mostly-empty space.
			const clusters = [...byTopic.entries()].map(([topic, indices]) => {
				const centroid = { x: 0, y: 0, z: 0 };
				for (const i of indices) {
					centroid.x += simNodes[i].x;
					centroid.y += simNodes[i].y;
					centroid.z += simNodes[i].z;
				}
				centroid.x /= indices.length;
				centroid.y /= indices.length;
				centroid.z /= indices.length;
				const dists = indices
					.map(i => Math.hypot(simNodes[i].x - centroid.x, simNodes[i].y - centroid.y, simNodes[i].z - centroid.z))
					.sort((a, b) => a - b);
				const rawRadius = dists[Math.floor(dists.length * 0.9)] * 1.15;
				return { topic, indices, centroid, rawRadius, radius: rawRadius };
			});

			// Pass 2: cap each radius by the distance to its nearest other
			// centroid, so a topic whose own nodes happen to spread wide
			// can't balloon its sphere into a neighboring topic's — without
			// this, two adjacent bubbles routinely overlapped since sizing
			// was purely "how spread out are THIS topic's own nodes,"
			// independent of how close the next topic's anchor happens to
			// be. 0.45 rather than 0.5 of the gap leaves a visible seam
			// between touching spheres instead of them just kissing exactly.
			for (const c of clusters) {
				let nearest = Infinity;
				for (const other of clusters) {
					if (other === c) continue;
					const d = Math.hypot(c.centroid.x - other.centroid.x, c.centroid.y - other.centroid.y, c.centroid.z - other.centroid.z);
					if (d < nearest) nearest = d;
				}
				c.radius = Math.max(Math.min(c.rawRadius, nearest * 0.45), 2.5);
			}

			let hue = 0;
			for (const { topic, indices, centroid, radius } of clusters) {
				// Golden-angle hue step: consecutive topics (insertion order,
				// not spatial order) land far apart on the color wheel, so
				// two spheres that end up adjacent in 3D still read as
				// visually distinct rather than blending into each other.
				hue = (hue + 0.61803398875) % 1;
				const color = new THREE.Color().setHSL(hue, 0.55, 0.55);
				const sphereGeometry = new THREE.SphereGeometry(radius, 20, 14);
				const sphereMaterial = new THREE.MeshBasicMaterial({
					color,
					transparent: true,
					opacity: 0.09,
					depthWrite: false,
				});
				const sphere = new THREE.Mesh(sphereGeometry, sphereMaterial);
				sphere.position.set(centroid.x, centroid.y, centroid.z);
				scene!.add(sphere);
				topicSphereObjs.push(sphere);

				const div = document.createElement('div');
				// Most topics follow ICM's own "category-project" naming
				// convention (context-icm, decisions-rtk, ...); "context"
				// is by far the most common category, so stripping just
				// that prefix shortens most labels without losing the
				// categories that actually distinguish one label from
				// another (decisions-, security-, errors-resolved, ...).
				div.textContent = `${topic.replace(/^context-/, '')} (${indices.length})`;
				div.title = topic;
				div.className =
					'px-1.5 py-0.5 rounded text-[11px] font-medium max-w-[180px] truncate pointer-events-none bg-black/70 text-slate-100 border border-white/10';
				const obj = new CSS2DObject(div);
				// Floats just above the sphere rather than at its exact
				// center, so the label doesn't sit visually buried inside
				// the cluster's own node dots.
				obj.position.set(centroid.x, centroid.y + radius * 1.1, centroid.z);
				// Bigger topics win the overlap-resolution pass in
				// renderFrame() below — a small topic's label disappearing
				// when it happens to project behind a big one is a much
				// smaller loss than the reverse.
				obj.userData.labelPriority = indices.length;
				scene!.add(obj);
				labelObjects.push(obj);
			}
		} else {
			const MAX_LABELS = 18;
			const importanceRank: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 };
			const labelIndices = graph.nodes
				.map((_, i) => i)
				.sort((a, b) => {
					const ra = importanceRank[graph.nodes[a].importance] ?? 4;
					const rb = importanceRank[graph.nodes[b].importance] ?? 4;
					if (ra !== rb) return ra - rb;
					return graph.nodes[b].weight - graph.nodes[a].weight;
				})
				.slice(0, MAX_LABELS);
			// labelIndices is already sorted best-first (critical > high >
			// ... > weight); reversing it into a priority number keeps that
			// same order in renderFrame()'s overlap-resolution pass below.
			labelIndices.forEach((i, rank) => {
				const node = graph.nodes[i];
				const div = document.createElement('div');
				div.textContent = truncateLabel(node.summary);
				div.title = node.summary;
				div.className =
					'px-1.5 py-0.5 rounded text-[10px] max-w-[160px] truncate pointer-events-none bg-black/60 text-slate-200';
				const obj = new CSS2DObject(div);
				obj.position.set(simNodes[i].x, simNodes[i].y, simNodes[i].z);
				obj.userData.labelPriority = labelIndices.length - rank;
				scene!.add(obj);
				labelObjects.push(obj);
			});
		}
	}

	function truncateLabel(s: string, max = 36): string {
		return s.length > max ? `${s.slice(0, max - 1)}…` : s;
	}

	// --- Render loop: node/edge/pulse positions are all set once in
	// buildScene() from the server-computed layout and never move (no
	// client-side physics to step — see buildSim above), so this loop's
	// only job is the continuous "information transiting" pulse animation
	// and camera damping. It intentionally never idles: the pulses need a
	// frame every tick to move. ---
	let rafId = 0;

	function renderFrame() {
		if (!scene || !camera || !renderer || !labelRenderer || !controls) return;
		controls.update();
		clock.update();
		const pulseMat = pulseObj?.material as THREE.ShaderMaterial | undefined;
		if (pulseMat) pulseMat.uniforms.u_time.value = clock.getElapsed();
		renderer.render(scene, camera);
		labelRenderer.render(scene, camera);
		resolveLabelOverlaps();
	}

	// CSS2DRenderer recomputes every label's on-screen transform from its
	// 3D position each frame, so there's no stable per-label pixel offset
	// to nudge — any manual repositioning would just be overwritten next
	// tick. Hiding instead of moving is simpler and correctly favors
	// keeping the more important label fully readable over cramming both
	// in illegibly: run after every render, since orbiting the camera can
	// change which labels' *projected* positions collide even though
	// their real 3D positions never move.
	function resolveLabelOverlaps() {
		const kept: DOMRect[] = [];
		const byPriority = [...labelObjects].sort(
			(a, b) => (b.userData.labelPriority ?? 0) - (a.userData.labelPriority ?? 0),
		);
		for (const obj of byPriority) {
			const el = obj.element;
			const rect = el.getBoundingClientRect();
			const overlapsKept = kept.some(
				r => rect.left < r.right && rect.right > r.left && rect.top < r.bottom && rect.bottom > r.top,
			);
			el.style.visibility = overlapsKept ? 'hidden' : '';
			if (!overlapsKept) kept.push(rect);
		}
	}

	function loop() {
		renderFrame();
		rafId = requestAnimationFrame(loop);
	}

	function kickRenderLoop() {
		// Once the pulse animation is running, the main loop is already
		// continuous — an extra one-shot frame would just be redundant.
		if (rafId === 0) {
			rafId = requestAnimationFrame(loop);
		}
	}

	function onPointerDown(e: PointerEvent) {
		pointerDownPos = { x: e.clientX, y: e.clientY };
	}
	function onPointerUp(e: PointerEvent) {
		if (!pointerDownPos || !renderer || !camera || !pointsObj || !raycaster) return;
		// Only treat this as a "click to select" if the pointer barely
		// moved — OrbitControls uses the same pointer events to orbit the
		// camera, so a drag must not also select whatever node happens to
		// be under the release point.
		const moved = Math.hypot(e.clientX - pointerDownPos.x, e.clientY - pointerDownPos.y);
		pointerDownPos = null;
		if (moved > 5) return;

		const rect = renderer.domElement.getBoundingClientRect();
		const ndc = new THREE.Vector2(
			((e.clientX - rect.left) / rect.width) * 2 - 1,
			-((e.clientY - rect.top) / rect.height) * 2 + 1,
		);
		raycaster.setFromCamera(ndc, camera);
		// A fixed world-space threshold only works at one zoom level — it
		// was tuned against a ~3-unit-wide demo layout, but a real store's
		// clusters now spread over hundreds of units (see graph_layout.rs's
		// CLUSTER_SPACING), so the same absolute threshold shrinks to a
		// near-invisible target once the camera is zoomed out to fit that.
		// Scaling it by the current camera-to-target distance keeps the
		// click tolerance roughly constant in screen space at any zoom.
		const camDist = controls ? camera.position.distanceTo(controls.target) : 10;
		raycaster.params.Points = { threshold: camDist * 0.01 };
		const hits = raycaster.intersectObject(pointsObj);
		selected = hits.length > 0 ? (graph.nodes[hits[0].index!] ?? null) : null;
	}

	// Sets the camera back far enough to see every node, based on the
	// *actual* extent of the layout the server returned, not a formula
	// guessing at it. An earlier version scaled the initial distance by
	// sqrt(nodeCount) — tuned against the 25-node demo dataset — but the
	// force layout's real equilibrium spread doesn't grow that predictably:
	// checked against a real 3286-node store, some nodes ended up ~250
	// units from the origin (two orders of magnitude past what the formula
	// assumed), so the camera and its maxDistance clamp both need to be
	// derived from the data itself.
	function fitCameraToNodes() {
		if (!camera || !controls || simNodes.length === 0) return;
		let maxR = 0;
		for (const n of simNodes) {
			const r = Math.hypot(n.x, n.y, n.z);
			if (r > maxR) maxR = r;
		}
		maxR = Math.max(maxR, 0.5); // floor so a 1-2 node graph isn't a close-up blur
		const fovRad = (camera.fov * Math.PI) / 180;
		// Distance so the farthest node's radius just fits the vertical FOV,
		// plus 30% padding so nothing sits right at the frame edge.
		const dist = (maxR / Math.sin(fovRad / 2)) * 1.3;
		// Oblique 3/4 view, not a dead-on (0,0,dist) look down the Z axis.
		// Cluster anchors are placed on a Fibonacci sphere (see
		// graph_layout.rs), and any point near the axis a camera looks
		// straight down collapses toward the center of the projected
		// image regardless of how far apart clusters really are in 3D —
		// roughly a third of a real store's topics ended up visually
		// piled in the middle of the frame this way even though their
		// computed positions were genuinely well separated. An oblique
		// angle has no axis the anchors' poles can degenerate onto.
		const dir = new THREE.Vector3(0.55, 0.4, 0.73).normalize();
		camera.position.copy(dir.multiplyScalar(dist));
		// The far clipping plane must comfortably exceed how far the camera
		// can be zoomed out (maxDistance below), or nodes past it are
		// invisibly clipped — happened for real at this graph's actual scale
		// with the constructor's fixed default (see its comment).
		camera.far = dist * 4;
		camera.updateProjectionMatrix();
		controls.maxDistance = dist * 3;
		controls.minDistance = Math.max(0.1, maxR * 0.02);
	}

	async function load() {
		loading = true;
		loadError = '';
		try {
			if (topics.length === 0) {
				topics = await api.topics();
			}
			// No client-side node cap: the layout is computed server-side now
			// (see buildSim's doc comment), so a large unfiltered store is a
			// slower request, not a hung tab — tested end-to-end against a
			// real 3286-memory store.
			graph = await api.graph(selectedTopic || undefined);
			showLinks = graph.edges.length <= 200;
			buildSim(graph);
			buildScene();
			fitCameraToNodes();
			if (rafId === 0) {
				rafId = requestAnimationFrame(loop);
			}
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	function resizeRenderer() {
		if (!containerEl || !renderer || !camera || !labelRenderer) return;
		const w = containerEl.clientWidth;
		const h = containerEl.clientHeight;
		if (w === 0 || h === 0) return;
		camera.aspect = w / h;
		camera.updateProjectionMatrix();
		renderer.setSize(w, h);
		labelRenderer.setSize(w, h);
		for (const obj of edgeTierObjs) {
			(obj.material as LineMaterial).resolution.set(w, h);
		}
		kickRenderLoop();
	}

	onMount(() => {
		scene = new THREE.Scene();
		// Far plane is generous (and re-derived per-graph in fitCameraToNodes)
		// because the force layout's real equilibrium spread scales far less
		// predictably than node count alone — a fixed 100 clipped an entire
		// 3286-node real store invisibly out of view.
		camera = new THREE.PerspectiveCamera(55, 1, 0.01, 2000);
		camera.position.set(0, 0, 3.4);

		renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
		renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
		renderer.domElement.className = 'absolute inset-0 w-full h-full cursor-grab active:cursor-grabbing';
		containerEl.appendChild(renderer.domElement);

		labelRenderer = new CSS2DRenderer();
		labelRenderer.domElement.className = 'absolute inset-0 pointer-events-none';
		containerEl.appendChild(labelRenderer.domElement);

		controls = new OrbitControls(camera, renderer.domElement);
		controls.enableDamping = true;
		controls.dampingFactor = 0.08;
		controls.minDistance = 0.6;
		controls.maxDistance = 12;
		controls.addEventListener('change', kickRenderLoop);

		raycaster = new THREE.Raycaster();

		renderer.domElement.addEventListener('pointerdown', onPointerDown);
		renderer.domElement.addEventListener('pointerup', onPointerUp);

		resizeRenderer();
		const resizeObserver = new ResizeObserver(resizeRenderer);
		resizeObserver.observe(containerEl);
		load();

		return () => {
			resizeObserver.disconnect();
			controls?.dispose();
			renderer?.dispose();
		};
	});

	onDestroy(() => {
		if (rafId) cancelAnimationFrame(rafId);
	});
</script>

<div class="flex flex-col h-full">
	<div class="flex items-center justify-between mb-4">
		<div class="flex items-center gap-3">
			<h2 class="text-2xl font-bold">Graph</h2>
			<select
				bind:value={selectedTopic}
				onchange={load}
				class="bg-[var(--card)] border border-[var(--border)] rounded px-2 py-1 text-xs"
			>
				<option value="">All topics</option>
				{#each topics as t (t.name)}
					<option value={t.name}>{t.name} ({t.count})</option>
				{/each}
			</select>
			<label class="flex items-center gap-1.5 text-xs text-[var(--muted)] cursor-pointer select-none">
				<input type="checkbox" bind:checked={showLinks} onchange={buildScene} />
				Show links
			</label>
		</div>
		<div class="text-xs text-[var(--muted)]">
			{#if !loading && !loadError}
				{graph.nodes.length} memories · {graph.edges.length} links · drag to orbit, scroll to zoom, click a node
			{/if}
		</div>
	</div>

	<!-- The canvas/renderer live inside containerEl, appended once by
	     onMount (three.js owns that DOM node directly, outside Svelte's
	     control). This div must therefore stay mounted for the component's
	     entire lifetime — swapping it out for an "empty state" block via an
	     {#if}/{:else} at this level (an earlier version of this page did
	     exactly that) detaches the canvas from the document the moment any
	     of loadError/empty/single-node becomes true, and it never
	     comes back even after the state clears, because onMount's
	     appendChild only ever runs once. All of those states are overlays
	     inside the permanent container instead. -->
	<div class="flex flex-1 gap-4 min-h-0">
		<div
			bind:this={containerEl}
			class="flex-1 relative bg-[var(--card)] rounded-lg border border-[var(--border)] overflow-hidden"
		>
			{#if loading}
				<div class="absolute inset-0 flex items-center justify-center text-[var(--muted)] text-sm">
					Loading graph...
				</div>
			{:else if loadError}
				<div class="absolute inset-0 flex items-center justify-center p-8">
					<div class="text-red-400 text-sm text-center">Failed to load graph: {loadError}</div>
				</div>
			{:else if graph.nodes.length === 0}
				<div class="absolute inset-0 flex items-center justify-center p-8">
					<div class="text-center text-[var(--muted)]">
						<p class="mb-1">No memories yet — nothing to graph.</p>
						<p class="text-xs">Store a few related memories and they'll auto-link here.</p>
					</div>
				</div>
			{:else if graph.nodes.length === 1}
				<div class="absolute inset-0 flex items-center justify-center p-8">
					<p class="text-center text-[var(--muted)]">
						Only one memory so far — links appear once there's something to relate it to.
					</p>
				</div>
			{/if}
		</div>

		<div class="w-72 shrink-0 bg-[var(--card)] rounded-lg border border-[var(--border)] p-4 overflow-y-auto">
				{#if selected}
					<div class="flex items-center gap-2 mb-2">
						<span class="text-xs px-1.5 py-0.5 rounded bg-[var(--border)]">{selected.topic}</span>
						<span class="text-xs" style="color: {
							selected.importance === 'critical' ? '#f87171' :
							selected.importance === 'high' ? '#fb923c' :
							selected.importance === 'medium' ? '#60a5fa' : '#9ca3af'
						}">{selected.importance}</span>
					</div>
					<p class="text-sm mb-3">{selected.summary}</p>
					<div class="text-xs text-[var(--muted)] space-y-1">
						<div>weight: {selected.weight.toFixed(3)}</div>
						<div class="font-mono">{selected.id}</div>
					</div>
					{#if linkedNodes.length > 0}
						<div class="mt-4">
							<div class="text-xs text-[var(--muted)] mb-1.5">
								Linked to ({linkedNodes.length}):
							</div>
							<ul class="space-y-1.5">
								{#each linkedNodes as link (link.node.id)}
									<li class="flex items-center justify-between gap-2 text-xs">
										<button
											class="truncate text-left hover:underline"
											title={link.node.summary}
											onclick={() => (selected = link.node)}
										>
											{truncateLabel(link.node.summary, 30)}
										</button>
										{#if link.similarity > 0}
											<span class="text-[var(--muted)] shrink-0 font-mono">
												{link.similarity.toFixed(2)}
											</span>
										{:else}
											<span
												class="text-[var(--muted)] shrink-0 italic"
												title="auto_link only creates a link above 0.75 real similarity, so a link showing exactly 0 isn't 'unrelated' — it means at least one of the two memories has no stored embedding (e.g. a --no-embeddings store) to compute a real score from."
											>
												unknown
											</span>
										{/if}
									</li>
								{/each}
							</ul>
						</div>
					{/if}
				{:else}
					<p class="text-sm text-[var(--muted)]">Click a node to see its details.</p>
					<div class="mt-4 text-xs text-[var(--muted)] space-y-1">
						<div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full" style="background:#f87171"></span>Critical</div>
						<div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full" style="background:#fb923c"></span>High</div>
						<div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full" style="background:#60a5fa"></span>Medium</div>
						<div class="flex items-center gap-2"><span class="w-2 h-2 rounded-full" style="background:#9ca3af"></span>Low</div>
						<div class="pt-2 text-[var(--muted)]">Node size also scales with importance and weight.</div>
					</div>
				{/if}
		</div>
	</div>
</div>
