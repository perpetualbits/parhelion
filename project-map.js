/* ==========================================================================
 * project-map.js — DATA for the Parhelion project map.
 *
 * This file is the single source of truth the renderer (project-map.html)
 * reads. It is intentionally pure data: `window.PROJECT_MAP` and nothing else.
 *
 * Status is DERIVED, never invented:
 *   - "done"    = code is in the tree with tests (verified via `make test`).
 *   - "active"  = part of the current milestone (M1) and not yet shipped.
 *   - "planned" = scheduled for a later milestone (M2..M9); no code yet.
 *   - "seam"    = a deliberate interface that exists now but is filled later
 *                 (e.g. the texture-source binding, the render-target trait).
 *
 * Sources: docs/parhelion_milestone_plan.md (roadmap), docs/scene_graph_v1.md,
 * docs/CORE-BOUNDARY.md, docs/parhelion_project_index.md, and the crate tree.
 * Keep this file in sync with the roadmap whenever status changes.
 * ========================================================================== */

window.PROJECT_MAP = {
  project: {
    name: "PARHELION",
    tagline:
      "A Wayland compositor built as a 3D-native scene-graph engine with microkernel discipline — a small realtime core ringed by isolated, restartable, capability-scoped processes.",
    repo: "github.com/perpetualbits/parhelion",
    updated: "2026-07-26",
  },

  // Four reserved states. Each ships with a glyph + label so meaning never
  // rests on colour alone (the palette is CVD-validated, but shape carries it).
  statuses: {
    done:    { label: "Shipped",     hint: "Built and tested — in the tree, green under `make test`." },
    active:  { label: "In progress", hint: "Part of the current milestone (M1); not yet shipped." },
    planned: { label: "Planned",     hint: "Scheduled for a later milestone; no code yet." },
    seam:    { label: "Seam",        hint: "A deliberate interface reserved now, filled by later work." },
  },

  // Architectural bands, drawn top (what connects) → bottom (what everything
  // rests on). Each node lives in exactly one band.
  layers: [
    { id: "edge",      label: "Edge",              hint: "Clients that speak to the core — local and remote." },
    { id: "protocol",  label: "Protocol frontend", hint: "Wayland machinery: dispatch, globals, surface lifecycle." },
    { id: "state",     label: "Canonical state",   hint: "The core-owned truth: scene graph, snapshots, control plane." },
    { id: "render",    label: "Render loop",       hint: "Snapshot → composite → submit; regimes and damage." },
    { id: "backend",   label: "Backends & input",  hint: "Where frames land and input comes from." },
    { id: "processes", label: "Microkernel processes", hint: "Isolated, restartable server processes around the core." },
    { id: "foundation",label: "Foundation",        hint: "The harness, the memory system, the vendored language." },
  ],

  nodes: [
    /* ---- Edge ------------------------------------------------------------ */
    {
      id: "wl-clients", label: "Wayland clients", layer: "edge", status: "active",
      tags: ["M1"],
      desc: "Ordinary applications speaking the Wayland protocol. A scripted in-process client drives the protocol rig, and real third-party clients connect over a real socket: foot — a terminal nobody wrote for us — maps, renders, and echoes typed input under the compositor, and typing redraws 0.62% of the output. That run is an automated headless test, so the milestone's claim is re-proved on every CI push rather than remembered from one afternoon.",
      files: ["crates/harness/src/protocol_rig.rs"],
      specs: [{ label: "milestone M1", href: "docs/parhelion_milestone_plan.md" }],
      parts: [
        { label: "Scripted rig client", status: "done", desc: "In-process Wayland client for deterministic protocol tests." },
        { label: "Real terminal client", status: "done", desc: "foot runs headlessly under the compositor and echoes typed input — the M1 acceptance test (T7)." },
        { label: "Real socket", status: "done", desc: "External clients connect over a bound Wayland socket; foot gets as far as font loading." },
      ],
      deps: ["protocol-host"],
    },
    {
      id: "rayland", label: "Rayland remote", layer: "edge", status: "planned",
      tags: ["M6"],
      desc: "A remote client rendering across the network via Rayland. To the core it is an ordinary Wayland client with a smaller capability grant; no core code path ever names Rayland. Attaches its pixels through the texture-source seam as a token buffer.",
      files: [],
      specs: [{ label: "VISION — Rayland host", href: "docs/VISION.md" }],
      parts: [],
      deps: ["texture-seam", "rayland-replay"],
    },

    /* ---- Protocol frontend ---------------------------------------------- */
    {
      id: "protocol-host", label: "ProtocolHost (shards=1)", layer: "protocol", status: "done",
      tags: ["M0"],
      desc: "The Wayland protocol frontend: a dispatch thread owning the Smithay Display, advertising wl_compositor, assigning each client to a shard at accept time, and publishing surface lifecycle to the scene by message. Structured so growing from one shard to many is an implementation change, not an architectural one.",
      files: ["crates/core/src/protocol.rs"],
      specs: [
        { label: "CORE-BOUNDARY §3 (C3), §7", href: "docs/CORE-BOUNDARY.md" },
        { label: "Smithay threading spike", href: "docs/smithay_threading_spike.md" },
      ],
      parts: [
        { label: "Accept seam / shard assignment", status: "done", desc: "add_client routes a socket to a shard." },
        { label: "wl_compositor / wl_surface", status: "done", desc: "create / commit / destroy via Smithay's compositor handler." },
        { label: "Send/Sync static guards", status: "done", desc: "Compile-time regression guards on the threading facts." },
        { label: "Per-client readiness", status: "done", desc: "One calloop source per client (M2 T0): throttling disables a source outright, ending the dispatch-loop spin by construction — and it is the shape a future shard owns." },
      ],
      deps: [],
    },
    {
      id: "buffers-shm", label: "wl_shm buffers", layer: "protocol", status: "done",
      tags: ["M1", "T3"],
      desc: "Shared-memory buffer attach / commit / release. At commit the dispatch thread copies + decodes the buffer into an owned pixel block and releases the wl_buffer immediately (single-buffer clients run; destroy-after-commit is safe by construction). The seam check PASSED: handled through smithay::wayland::shm with no smithay::backend::renderer type — grep-verifiable.",
      files: ["crates/core/src/protocol.rs"],
      specs: [
        { label: "scene_graph_v1.md §3.1", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T3", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Copy-at-commit + release", status: "done", desc: "Dispatch-thread memcpy (off the frame path, I-1); immediate release." },
        { label: "argb8888 / xrgb8888", status: "done", desc: "Decoded to RGBA; format folded into the node's opaque flag." },
        { label: "Seam check", status: "done", desc: "No renderer types imported; the frontend/renderer split held." },
      ],
      deps: ["protocol-host", "texture-seam"],
    },
    {
      id: "xdg-shell", label: "xdg-shell toplevel", layer: "protocol", status: "done",
      tags: ["M1", "T5"],
      desc: "The window lifecycle: xdg_wm_base / xdg_surface / xdg_toplevel with the configure/ack dance, map/unmap, roles, and title/app_id captured into scene state. With it came the mapping-semantics migration — a surface without a role is never displayed, so only mapped toplevels (and core-injected C10 content) composite. Default placement is a deterministic cascade from the core's built-in fallback until a policy daemon exists (M4). Popups, decorations, and window states are out of scope until after M1.",
      files: ["crates/core/src/protocol.rs", "crates/core/src/scene/node.rs"],
      specs: [
        { label: "scene_graph_v1.md §10", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T5", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Configure/ack dance", status: "done", desc: "Initial buffer-less commit earns a 0×0 configure; a buffer before the ack is a protocol error." },
        { label: "Map / unmap", status: "done", desc: "First buffer commit maps; null attach or role destroy unmaps, with structural damage and the dance re-armed." },
        { label: "Roles + metadata", status: "done", desc: "NodeRole gates visibility; title/app_id captured into canonical state (nothing branches on them yet)." },
        { label: "C10 cascade placement", status: "done", desc: "Deterministic per-toplevel offset from named constants — temporary until S1 (M4)." },
        { label: "Ping / pong", status: "done", desc: "Liveness mechanism; the scheduler and unresponsive-client policy are S1's (M4)." },
      ],
      deps: ["protocol-host", "scene-graph"],
    },
    {
      id: "data-device", label: "Clipboard & DnD", layer: "protocol", status: "done",
      tags: ["M1", "T7"],
      desc: "wl_data_device_manager: the clipboard, and a deliberately deferred drag-and-drop. Not a shell feature — a service the display server owes every client, and real applications treat its absence as a broken compositor (foot refuses to start). The bytes never touch the compositor: a copy publishes a source, a paste asks the offer for a pipe, and the clients transfer directly. Focus-gating IS the v1 capability model and satisfies I-7's letter; the deeper design (security contexts, remote-client grants) is M4's C8 work. Proven end to end by wl-copy → wl-paste, two third-party programs.",
      files: ["crates/core/src/protocol.rs"],
      specs: [{ label: "scene_graph_v1.md §12.2", href: "docs/scene_graph_v1.md" }],
      parts: [
        { label: "Clipboard (selection)", status: "done", desc: "Client-to-client transfer through a pipe; focus gate asserted by test, not assumed." },
        { label: "Selection liveness", status: "done", desc: "The owner's death clears the clipboard — checked after its teardown, or the dying offer is re-broadcast." },
        { label: "Drag-and-drop", status: "seam", desc: "Refused honestly: start_drag cancels the source at once. Real DnD is a pointer grab, and how grabs meet the focus model is its own design conversation." },
        { label: "Capability gating (I-7)", status: "planned", desc: "Focus-gated now; security contexts and remote-client grants arrive with C8 (M4)." },
      ],
      deps: ["protocol-host", "seat-input"],
    },
    {
      id: "subsurfaces", label: "Subsurfaces", layer: "protocol", status: "done",
      tags: ["M2", "T7"],
      desc: "wl_subcompositor honoured: subsurfaces are scene nodes with a parent, a parent-relative position, and a place in their sibling order (the children list carries the parent's own slot, because place_below puts a child BENEATH its parent). Mapping is transitive — a subsurface composites only while its whole ancestor chain does — and synchronized commits land as one atomic scene message, so a window and its decorations are never seen half-updated. The snapshot flattens the tree, so the renderer is unchanged: still a flat back-to-front list. foot has its decorations back; the debt T7b mis-measured and T0 pinned is discharged.",
      files: ["crates/core/src/protocol.rs"],
      specs: [{ label: "scene_graph_v1.md §12.3", href: "docs/scene_graph_v1.md" }],
      parts: [
        { label: "Scene tree", status: "done", desc: "Parent links, parent-relative transforms, sibling order with the parent's own slot, arbitrary nesting (depth-guarded)." },
        { label: "Sync / desync", status: "done", desc: "One effective commit → one atomic scene message; the golden pair pins both frames." },
        { label: "Damage through the tree", status: "done", desc: "Subtree old ∪ new on moves and restacks; the equivalence oracle covers the tree steps." },
        { label: "Input hit-testing", status: "done", desc: "Child-local coordinates; subsurfaces take pointer but never keyboard focus; pixel-less children are click-transparent." },
        { label: "Viewporter / scale", status: "planned", desc: "Buffer scale and viewport on subsurfaces (M2+)." },
      ],
      deps: ["scene-graph"],
    },
    {
      id: "wl-output", label: "wl_output", layer: "protocol", status: "done",
      tags: ["M1", "T7"],
      desc: "The screen a client asks about before it draws: one output with the backend's real size, 60 Hz, scale 1, zero physical size (a nested window has no millimetres), and wl_surface.enter/leave as windows map and unmap. Implemented properly rather than stubbed — an advertised-but-hollow global is a lie to every future client. xdg_output rides alongside with the logical geometry.",
      files: ["crates/core/src/protocol.rs"],
      specs: [{ label: "scene_graph_v1.md §12.1", href: "docs/scene_graph_v1.md" }],
      parts: [
        { label: "Mode + scale + done", status: "done", desc: "Real values from the backend, re-advertised on resize." },
        { label: "Surface enter/leave", status: "done", desc: "Idempotent, following map and unmap." },
        { label: "xdg_output", status: "done", desc: "Logical geometry, tested alongside." },
      ],
      deps: ["protocol-host"],
    },
    {
      id: "seat-input", label: "wl_seat (kbd + pointer)", layer: "protocol", status: "done",
      tags: ["M1", "T6"],
      desc: "Input delivery to the focused client: keyboard and pointer via wl_seat, with focus following the topmost mapped toplevel as a temporary core fallback. Every source — the nested backend, the test rig, and later libinput — produces the same InputEvent and hands it to the dispatch thread, which owns the seat. Routing reads a dispatch-side replica of the scene's geometry rather than querying the scene, so input never waits on rendering (I-2).",
      files: ["crates/core/src/input.rs", "crates/core/src/protocol.rs"],
      specs: [
        { label: "scene_graph_v1.md §11", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T6", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Seat + keymap", status: "done", desc: "wl_seat with keyboard and pointer capabilities; an xkb 'us' keymap delivered to each client." },
        { label: "The input funnel", status: "done", desc: "One InputEvent enum, evdev codes, no Smithay type — a non-blocking message into the dispatch thread." },
        { label: "Focus routing table", status: "done", desc: "Read-mostly replica of rect + stacking per mapped surface; ordering matches the snapshot's draw order." },
        { label: "Focus policy (C10)", status: "done", desc: "Keyboard follows the topmost mapped toplevel; pointer follows the cursor. Temporary — S1 takes over in M4." },
        { label: "Cursor surfaces", status: "planned", desc: "set_cursor is accepted and ignored for rendering; the cursor plane is M2." },
      ],
      deps: ["protocol-host", "xdg-shell"],
    },
    {
      id: "frame-callbacks", label: "Frame callbacks & flush", layer: "protocol", status: "done",
      tags: ["M1", "T2"],
      desc: "The reverse direction: wl_surface.frame callbacks fired from the render side when a frame is presented, flush ownership settled (the dispatch thread flushes once per loop; the render side only enqueues), and the backpressure policy so a flooding client cannot stall its shard-mates. The render→dispatch notice is a wait-free atomic timestamp + a calloop ping (I-1); every Wayland object stays on the dispatch thread (§7).",
      files: ["crates/core/src/protocol.rs", "crates/core/src/render.rs"],
      specs: [
        { label: "scene_graph_v1.md §8", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T2", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Frame-presented notice", status: "done", desc: "FramePresenter: atomic timestamp + ping, coalescing and single-slot bounded." },
        { label: "wl_surface.frame → done", status: "done", desc: "Dispatch thread drains committed callbacks per tick; v1 fires all pending (occlusion gating is M2)." },
        { label: "Single flush site", status: "done", desc: "One flush_clients per loop iteration; grep-verifiable." },
        { label: "Per-client backpressure", status: "done", desc: "Pending-callback cap; over-bound clients unscheduled, not killed (I-10)." },
      ],
      deps: ["protocol-host", "render-loop"],
    },

    /* ---- Canonical state ------------------------------------------------- */
    {
      id: "scene-graph", label: "Scene graph v1", layer: "state", status: "done",
      tags: ["M1", "T1"],
      desc: "The canonical state (I-5): every live surface as a node with placement, size, stacking, texture source, and opacity, owned by one dedicated scene thread and reached only by message. Born 3D-ready (a transform slot and texture-source binding from day one) but implemented 2.5D (only the axis-aligned integer path is reachable). Absorbed the M0 ledger.",
      files: ["crates/core/src/scene/"],
      specs: [
        { label: "scene_graph_v1.md", href: "docs/scene_graph_v1.md" },
        { label: "CORE-BOUNDARY §3 (C4)", href: "docs/CORE-BOUNDARY.md" },
      ],
      parts: [
        { label: "Node model + Transform", status: "done", desc: "Extensible enum: Identity/Translate now, 3D variants later." },
        { label: "Scene thread ownership (§7)", status: "done", desc: "SceneThread owns the state; SceneHandle is the only door." },
        { label: "Surface lifecycle", status: "done", desc: "create / commit / destroy / client-gone folded from protocol events." },
        { label: "Damage tracking", status: "done", desc: "Per-surface damage + region algebra, drained into the snapshot (T4)." },
        { label: "Roles + mapping rule", status: "done", desc: "A node is displayed only with a role (toplevel or core-owned), a source, and a non-empty size (T5)." },
      ],
      deps: ["protocol-host"],
    },
    {
      id: "snapshot", label: "Snapshot mechanism", layer: "state", status: "done",
      tags: ["M1", "T1"],
      desc: "The immutable, owned, back-to-front value that is the only way the scene crosses to the render thread — so no lock is ever shared with the frame path (I-1). v1 is a full copy of the visible-node list; persistent structural sharing stays a deliberately open question.",
      files: ["crates/core/src/scene/snapshot.rs"],
      specs: [{ label: "scene_graph_v1.md §5", href: "docs/scene_graph_v1.md" }],
      parts: [],
      deps: ["scene-graph"],
    },
    {
      id: "texture-seam", label: "Texture-source seam", layer: "state", status: "seam",
      tags: ["Rayland", "C9"],
      desc: "The single point where the scene decides what a node is textured with, carrying the rule that nothing may assume pixels are locally produced. Ships Solid and Shm now (Shm as a source-neutral PixelBuffer, so the renderer never learns its origin); dmabuf and the Rayland token buffer attach here as later work fills them in.",
      files: ["crates/core/src/scene/node.rs"],
      specs: [{ label: "scene_graph_v1.md §3", href: "docs/scene_graph_v1.md" }],
      parts: [
        { label: "Solid colour", status: "done", desc: "Test source + built-in C10 fallbacks." },
        { label: "Shm", status: "done", desc: "Real (T3): a wl_shm buffer decoded into a source-neutral pixel block." },
        { label: "dmabuf", status: "planned", desc: "Local GPU buffer import (M2)." },
        { label: "Rayland token buffer", status: "planned", desc: "S-side remote source (M6, CORE-BOUNDARY C9)." },
      ],
      deps: ["scene-graph"],
    },
    {
      id: "damage", label: "Damage tracking", layer: "state", status: "done",
      tags: ["M1", "T4"],
      desc: "Per-surface damage accumulation and region algebra (surface → scene → output coordinates), so a small commit redraws a proportionally small region. Conservative, bounded (coalesces to a bbox), subtraction-free; content-vs-structural split; damage-aware partial buffer copy with copy-on-write isolation. Damage changes cost, never output — the governing property (incremental == from-scratch) has its own test.",
      files: ["crates/core/src/scene/region.rs", "crates/core/src/scene/state.rs"],
      specs: [
        { label: "scene_graph_v1.md §9", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T4", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Region algebra", status: "done", desc: "Rect/Region: union/translate/clip, coalesce-to-bbox past a threshold; no subtraction." },
        { label: "Scene-side damage", status: "done", desc: "Client damage + structural changes + full-output fallback, drained into the snapshot." },
        { label: "Retained-frame renderer", status: "done", desc: "Recompute only damaged rects over the previous frame." },
        { label: "Partial copy + CoW", status: "done", desc: "Copy only the damaged buffer region; Arc::make_mut keeps snapshots isolated." },
      ],
      deps: ["scene-graph"],
    },
    {
      id: "control-plane", label: "Control plane (SPINE C7)", layer: "state", status: "planned",
      tags: ["M3"],
      desc: "The declarative control plane: a dialect of ENO's SPINE language over a JSON socket, with a C7 interpreter that runs animation programs on the core's own clock. Servers describe intent (target states, springs, timelines); the core executes it. The dialect crate is a skeleton today.",
      files: ["crates/dialect/"],
      specs: [{ label: "desktop dialect spec", href: "docs/parhelion_desktop_dialect.md" }],
      parts: [],
      deps: ["scene-graph"],
    },

    /* ---- Render loop ----------------------------------------------------- */
    {
      id: "render-loop", label: "Render loop (T-render)", layer: "render", status: "done",
      tags: ["M1", "T1"],
      desc: "The render skeleton: pull an immutable snapshot, hand it to the compositor, and count the frame. Driven by a test-controlled tick today (no wall-clock, deterministic goldens); the vblank-tied frame scheduler that replaces the tick arrives with the DRM backend.",
      files: ["crates/core/src/render.rs"],
      specs: [{ label: "scene_graph_v1.md §4", href: "docs/scene_graph_v1.md" }],
      parts: [
        { label: "Tick + frame counters", status: "done", desc: "frames-produced / nodes-composited instrumentation." },
        { label: "Frame scheduler (vblank)", status: "planned", desc: "Render-as-late-as-possible, tied to T-commit (M2)." },
      ],
      deps: ["snapshot", "compositor-seam"],
    },
    {
      id: "compositor-seam", label: "Compositor seam", layer: "render", status: "seam",
      tags: ["C5→C1"],
      desc: "A one-method trait the core defines and drives, so the core names no backend and no frame type. The headless CPU compositor implements it now; M2's DRM/GPU renderer implements the same trait. This is the seam that keeps Frame out of the core.",
      files: ["crates/core/src/render.rs"],
      specs: [{ label: "scene_graph_v1.md §6", href: "docs/scene_graph_v1.md" }],
      parts: [],
      deps: ["render-loop"],
    },
    {
      id: "cpu-compositor", label: "CPU compositor v1", layer: "render", status: "done",
      tags: ["M1", "T1", "T3", "T4"],
      desc: "Paints a snapshot into an in-memory frame, integer-only and tolerance-0. Blits both solid colours and shm pixel blocks through one clip + integer source-over path — it knows nothing about 'shm', just a PixelBuffer (T3). Retains the previous frame and recomputes only the snapshot's damage rects (T4); pixels outside damage keep their retained values. Lives in the backend crate alongside the Frame it renders, so the core depends on no backend.",
      files: ["crates/backend-headless/src/composite.rs"],
      specs: [{ label: "scene_graph_v1.md §6", href: "docs/scene_graph_v1.md" }],
      parts: [],
      deps: ["compositor-seam", "snapshot"],
    },
    {
      id: "gpu-renderer", label: "GPU renderer + dmabuf", layer: "render", status: "planned",
      tags: ["M2"],
      desc: "The real renderer: GPU-backed compositing with dmabuf import and explicit sync as the primary path (I-11). Implements the compositor seam for hardware, replacing the CPU compositor on the metal.",
      files: [],
      specs: [{ label: "milestone M2", href: "docs/parhelion_milestone_plan.md" }],
      parts: [],
      deps: ["compositor-seam", "drm"],
    },
    {
      id: "regime-machine", label: "Regime machine (2.5D ↔ 3D)", layer: "render", status: "planned",
      tags: ["M8"],
      desc: "The engineering crown jewel: a per-output state machine that flips to game-style full-frame rendering when transforms make damage non-local, then collapses back to the cheap damage-tracked regime within two frames of the last animation ending (I-9).",
      files: [],
      specs: [{ label: "VISION — Thesis 3", href: "docs/VISION.md" }],
      parts: [],
      deps: ["render-loop"],
    },

    /* ---- Backends & input ------------------------------------------------ */
    {
      id: "headless", label: "Headless backend", layer: "backend", status: "done",
      tags: ["M0"],
      desc: "In-memory rendering for deterministic tests: a tightly-packed RGBA8 Frame and the integer-only test pattern the golden rig was built on. The velocity multiplier — nothing lands without a headless-verifiable test.",
      files: ["crates/backend-headless/src/lib.rs"],
      specs: [{ label: "harness_design.md", href: "docs/harness_design.md" }],
      parts: [],
      deps: [],
    },
    {
      id: "winit", label: "winit nested backend", layer: "backend", status: "done",
      tags: ["M1", "T6"],
      desc: "A development backend that presents the core's CPU frames in a window and feeds window input into the input path — a window Roland can actually see, without needing real hardware. Raw winit + softbuffer, deliberately not Smithay's winit backend (which is welded to the renderer layer we bypass). Ships parhelion-dev: the window plus a real Wayland socket for external clients.",
      files: ["crates/backend-winit/"],
      specs: [
        { label: "scene_graph_v1.md §11.4", href: "docs/scene_graph_v1.md" },
        { label: "M1 tasks — T6", href: "docs/plans/m1_tasks.md" },
      ],
      parts: [
        { label: "Window + presentation", status: "done", desc: "softbuffer blit of the retained frame; one documented channel-order conversion." },
        { label: "Input translation", status: "done", desc: "winit keys → evdev (unmapped keys counted, never fatal), buttons, motion, and scroll into the funnel." },
        { label: "Resize", status: "done", desc: "Window resize → output resize + full damage; the compositor holds the repaint guarantee itself." },
        { label: "parhelion-dev + socket", status: "done", desc: "The thin dev binary; the listening socket lives in the core so it is testable without a display." },
        { label: "Graceful shutdown + headless", status: "done", desc: "SIGINT/SIGTERM exit through the normal path so the socket unlinks; --headless makes that testable in CI (M1 T7)." },
      ],
      deps: ["cpu-compositor", "seat-input"],
    },
    {
      id: "t-input", label: "T-input thread", layer: "backend", status: "seam",
      tags: ["M1", "M2", "§7"],
      desc: "CORE-BOUNDARY §7 gives input its own thread. M1 has its INTERFACE but not its thread: winit's event loop must own the main thread and delivers input intake and presentation together, so in the nested backend that loop is T-input. The funnel it feeds is already the seam a real T-input pushes into — libinput on its own thread (M2) becomes a third producer of an existing shape, and nothing downstream changes. A bounded, recorded deviation, not a redesign.",
      files: ["crates/core/src/input.rs"],
      specs: [
        { label: "scene_graph_v1.md §11.2", href: "docs/scene_graph_v1.md" },
        { label: "CORE-BOUNDARY §7", href: "docs/CORE-BOUNDARY.md" },
      ],
      parts: [
        { label: "InputEvent funnel", status: "done", desc: "The interface: every source produces it, the dispatch thread applies it." },
        { label: "Dedicated thread", status: "planned", desc: "Arrives with libinput and the DRM backend (M2)." },
      ],
      deps: ["seat-input"],
    },
    {
      id: "drm", label: "DRM/KMS + libinput", layer: "backend", status: "planned",
      tags: ["M2"],
      desc: "The metal: atomic KMS commits, plane assignment, mode setting, and a hardware cursor plane driven straight from the input thread; libinput for real devices; VT switching and modeset survival.",
      files: [],
      specs: [{ label: "milestone M2", href: "docs/parhelion_milestone_plan.md" }],
      parts: [],
      deps: [],
    },

    /* ---- Microkernel processes ------------------------------------------- */
    {
      id: "supervisor", label: "Supervisor (P0)", layer: "processes", status: "planned",
      tags: ["M4"],
      desc: "A minimal init-like process that spawns, monitors, and rate-limited-restarts the core and every server. Kept small enough to read in one sitting — small enough not to have bugs.",
      files: ["crates/supervisor/"],
      specs: [{ label: "CORE-BOUNDARY §6, §8", href: "docs/CORE-BOUNDARY.md" }],
      parts: [],
      deps: [],
    },
    {
      id: "policyd", label: "Policy daemon (S1)", layer: "processes", status: "planned",
      tags: ["M4"],
      desc: "The reference window-management daemon: placement, focus rules, and minimal tiling, speaking only the declarative control plane. It decides what; the core executes how. Kill it and the core carries on with fallbacks, then it resyncs.",
      files: ["crates/policyd/"],
      specs: [{ label: "CORE-BOUNDARY §6 (S1)", href: "docs/CORE-BOUNDARY.md" }],
      parts: [],
      deps: ["control-plane"],
    },
    {
      id: "shell", label: "Shell clients", layer: "processes", status: "planned",
      tags: ["M4"],
      desc: "Panel, launcher, wallpaper, notifications — ordinary layer-shell clients anyone can replace. Their crashes are cosmetic; the session survives.",
      files: [],
      specs: [{ label: "CORE-BOUNDARY §6 (S2)", href: "docs/CORE-BOUNDARY.md" }],
      parts: [],
      deps: ["policyd"],
    },
    {
      id: "rayland-replay", label: "Rayland replay (R1)", layer: "processes", status: "planned",
      tags: ["M6"],
      desc: "The sandboxed replay service that turns a remote command stream into pixels: seccomp-confined, its own render node, VRAM and rate quotas, a GPU-reset watchdog. The core touches only the resulting token buffer and syncobj (I-8).",
      files: [],
      specs: [{ label: "CORE-BOUNDARY §6 (R1), I-8", href: "docs/CORE-BOUNDARY.md" }],
      parts: [],
      deps: ["texture-seam"],
    },
    {
      id: "wasm-host", label: "WASM extension host (W1)", layer: "processes", status: "planned",
      tags: ["M9"],
      desc: "Third-party extensions as capability-scoped WASM components, preempted at their time budget — an extension that spins is suspended, not obeyed. Third-party code in the core process is structurally impossible.",
      files: [],
      specs: [{ label: "CORE-BOUNDARY §6 (W1)", href: "docs/CORE-BOUNDARY.md" }],
      parts: [],
      deps: ["control-plane"],
    },

    /* ---- Foundation ------------------------------------------------------ */
    {
      id: "harness", label: "Test harness", layer: "foundation", status: "done",
      tags: ["M0"],
      desc: "The golden-screenshot rig (render → compare bit-exactly against a committed PNG, fail with artifacts) and the protocol rig (drive a real ProtocolHost with a scripted client, assert on scene state). Ships a meta-test that proves the rig can fail.",
      files: ["crates/harness/"],
      specs: [{ label: "harness_design.md", href: "docs/harness_design.md" }],
      parts: [
        { label: "Golden rig", status: "done", desc: "Tolerance-0 PNG comparison + blessing workflow." },
        { label: "Protocol rig", status: "done", desc: "Scripted in-process client → scene assertions." },
        { label: "Scene-render goldens", status: "done", desc: "Stacking, clipping, snapshot isolation (M1 T1)." },
        { label: "Toplevel dance helper", status: "done", desc: "map_toplevel drives the whole xdg mapping sequence, so every test reads as 'a window' (M1 T5)." },
        { label: "Protocol-error assertions", status: "done", desc: "Tests pin the specific error code, never merely a disconnect (M1 T5)." },
        { label: "Input injection", status: "done", desc: "Scripted seat events through the production funnel; one ordered event log, because ordering is what the tests check (M1 T6)." },
        { label: "Socket tests", status: "done", desc: "A real client over a real listening socket — the dev binary's plumbing, verified without a display (M1 T6)." },
        { label: "Conformance sweep", status: "done", desc: "Every advertised global has an error-path or conformance test; the one gap found (wl_subcompositor) is reported, not papered over (M1 T7)." },
      ],
      deps: [],
    },
    {
      id: "docs-memory", label: "Docs & decision log", layer: "foundation", status: "done",
      tags: ["M0"],
      desc: "The project's memory: a founding vision, the normative core-boundary spec with its numbered invariants, an append-only decision log, a running diary, and per-session summaries. Documents are authoritative; code is downstream.",
      files: ["docs/"],
      specs: [
        { label: "VISION.md", href: "docs/VISION.md" },
        { label: "decision log", href: "docs/parhelion_decision_log.md" },
        { label: "project index", href: "docs/parhelion_project_index.md" },
      ],
      parts: [],
      deps: [],
    },
    {
      id: "spine-vendored", label: "SPINE core (vendored)", layer: "foundation", status: "seam",
      tags: ["upstream"],
      desc: "The pinned, read-only copy of ENO's SPINE language spec. ENO is upstream and evolves freely; core changes enter Parhelion only by deliberate, logged import. Shared language, separate runtimes.",
      files: ["third_party/spine/"],
      specs: [{ label: "dialect §0.1 (decoupling)", href: "docs/parhelion_desktop_dialect.md" }],
      parts: [],
      deps: [],
    },
  ],

  // The milestone sequence (phases) plus the CI invariant suites that harden
  // it (they enter at the milestone that makes them meaningful and never leave).
  roadmap: [
    { id: "m0", kind: "phase", label: "M0 · Skeleton & harness",     status: "done" },
    { id: "m1", kind: "phase", label: "M1 · One window, honestly",   status: "done" },
    { id: "m2", kind: "phase", label: "M2 · On the metal",           status: "active" },
    { id: "m3", kind: "phase", label: "M3 · Control plane",          status: "planned" },
    { id: "m4", kind: "phase", label: "M4 · Microkernel for real",   status: "planned" },
    { id: "m5", kind: "phase", label: "M5 · Efficiency",             status: "planned" },
    { id: "m6", kind: "phase", label: "M6 · Rayland hosting",        status: "planned" },
    { id: "m7", kind: "phase", label: "M7 · Shaped windows",         status: "planned" },
    { id: "m8", kind: "phase", label: "M8 · The third dimension",    status: "planned" },
    { id: "m9", kind: "phase", label: "M9 · Citizenship",            status: "planned" },

    { id: "h-golden", kind: "harden", label: "Golden rig — proven able to fail", status: "done" },
    { id: "h-damage", kind: "harden", label: "Damage counters (I-9 seed)",       status: "done" },
    { id: "h-stall",  kind: "harden", label: "Stall test — hostile daemon (I-1)", status: "planned" },
    { id: "h-crash",  kind: "harden", label: "Crash-only suite (I-5)",           status: "planned" },
    { id: "h-regime", kind: "harden", label: "Regime collapse ≤2 frames (I-9)",  status: "planned" },
  ],
};
