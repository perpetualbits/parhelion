# Parhelion — Vision & Principles

> **Re-entrancy header** — read this block to reconstitute context.
> **Status:** Draft v0.1 · **Date:** 2026-07-23 · **Kind:** P1 — founding vision, governs all other documents.
> **What this is:** the why and the non-negotiables of the Parhelion compositor. Every design doc, ADR, and task description is downstream of this file. When code and this file disagree, this file wins until amended.
> **Working name:** "Parhelion" is a placeholder (see §7).
> **Companion documents:** `CORE-BOUNDARY.md` (what lives in the core process and why), `decisions/` (append-only decision log, ADR-style), `DIARY.md` (session diary).
> **Sibling project:** [Rayland](https://github.com/perpetualbits/rayland) — native remote GPU rendering for Wayland. Parhelion is its reference S-side host.

---

## 1. What this is

Parhelion is a **Wayland-speaking 3D scene-graph engine with microkernel discipline**. It is a compositor built from a small, realtime-capable core surrounded by isolated, restartable, capability-scoped components — the way QNX built an operating system, and the way QNX's Photon built a GUI.

It exists to be three things at once, because they turn out to be one thing:

1. **The reference S-side for Rayland.** Rayland requires a compositor that hosts a remoting renderer: accepts buffers by token, shares a GPU allocator namespace with a sandboxed replay service, and waits on S-local sync objects. In Parhelion this is not a patch — it is a first-class buffer source in the scene graph. A scene node does not know whether its texture came from a local dmabuf, shared memory, or a Rayland replay service on behalf of a RISC-V board across the network.

2. **A desktop where windows are not condemned to be rectangles.** The scene graph is 3D-native from day one. A conventional window is the *degenerate case*: a textured quad, axis-aligned, fully opaque. Shaped windows and true 3D objects are the general case the architecture is built around — even though the degenerate case is the one that must be executed flawlessly and cheaply (see §4, Thesis 3).

3. **A desktop that cannot be stalled or crashed by its own extensions.** The founding grievance: one faulty GNOME extension stuttering an entire session, because gnome-shell runs compositor, window manager, shell UI, and third-party extensions in one process, one thread, one JavaScript heap. In Parhelion, third-party code in the core process is *structurally impossible*, not discouraged.

## 2. The one principle

> **Ship intent, not results. Let the receiver execute with its own resources, on its own clock.**

This single principle is enforced at every boundary of the system:

- **The network boundary (Rayland).** Transport the *language* of rendering — a command stream — not pixels. Where bulk data must move, transport a *descriptor* (content hash, provenance hint, URL) and let S fetch over its own fast path. Video is the canonical win: a URL crosses the thin link, S streams the content itself. Pixels are the fallback, never the goal.
- **The process boundary (control plane).** Policy daemons do not execute animations; they *describe* them — curves, timelines, spring parameters, target states — and the core interpolates per frame on its own realtime-scheduled clock. A daemon decides *what*; the core executes *how*. A slow daemon can therefore delay a *decision*, never a *frame*.
- **The extension boundary (sandbox).** Extensions submit declarative intent through capability-scoped interfaces and are preemptible at their time budget. An extension that spins is suspended, not obeyed.
- **The display boundary (KMS).** Atomic modesetting is itself intent-shipping: the core describes the desired plane configuration; the kernel executes it. Parhelion leans into this by aggressively demoting content to hardware planes.

A system with one principle applied at every boundary can be learned once and reviewed everywhere. Any proposed feature that requires shipping *results* across a boundary (pixels over the network, per-frame callbacks into external code, synchronous queries on the frame path) must justify itself against this principle in a decision-log entry — and will usually lose.

## 3. Origin and lineage

- **X11** proved network transparency, then lost it when toolkits went client-side; its flat trust model (any client may keylog any other) is the anti-pattern for our capability model.
- **Wayland** got buffers, isolation, and every-frame-perfect right, and deliberately excluded remoteness. Parhelion keeps Wayland's boundary and hosts Rayland *beside* it, exactly as GL sits beside Wayland today.
- **QNX / Photon microGUI** proved a GUI can be a tiny message-passing server with everything else as expendable processes. Parhelion is that thesis, restated for Wayland, Vulkan, and Rust.
- **GNOME Shell** is the cautionary tale: architecture, not code quality, determines whether an extension can stall a frame.
- **Compiz / Project Looking Glass / StardustXR / SimulaVR** are the 3D-desktop ancestors and living relatives; they prove the rendering and input problems are solvable and warn where damage tracking dies (§4, Thesis 3).
- **Arcan, River** prove crash-only desktop components and out-of-process policy are practical today.

## 4. Theses

**Thesis 1 — The three goals are one architecture.**
A retained-mode 3D scene graph whose texture sources are pluggable *is* the Rayland host, *is* the shaped-window renderer, *is* the 3D desktop. Building any one of the three properly builds the skeleton of the other two.

**Thesis 2 — Process boundaries follow fault, timing, and privilege — not memory.**
Rust provides memory safety between in-process modules for free, so we do not pay IPC costs for it. A component is exiled from the core if and only if it can block, can crash independently of the session's survival, carries third-party code, or parses hostile input. The exact placement rules are `CORE-BOUNDARY.md` §4.

**Thesis 3 — Two regimes, and the cheap one is sacred.**
The desktop lives ~98% of its life as axis-aligned opaque quads. In that regime Parhelion behaves like the most boring, most efficient 2D compositor imaginable: region-algebra damage tracking, scissored partial renders, hardware-plane offload, direct scanout, GPU asleep between events. Only when 3D transforms, shaped shadows, or screen-space effects make damage non-local does the affected output flip to game-style full-frame rendering — and there, the game industry's toolkit (depth prepass, occlusion culling from declared opaque geometry, cached shadow atlases) applies. The engineering crown jewel is the **state machine that re-collapses to the cheap regime within a frame or two** of the last animation ending. Battery life and idle wattage are correctness criteria, not nice-to-haves.

**Thesis 4 — Canonical state lives in the core; everything else is a restartable view.**
The core owns the truth: which surfaces exist, their geometry, their capability grants. Policy daemons, shell components, and effect services hold only derived state and can be killed and restarted at any moment, re-syncing from the core. This makes the desktop *crash-only* rather than crash-resistant: recovery is the ordinary code path, not the exceptional one.

**Thesis 5 — One capability mechanism, many trust levels.**
A local app, a shell panel, the window-management daemon, and a remote Rayland client are all just Wayland clients with different capability grants, tagged via `wp_security_context_v1` and enforced in the core. Remote clients get *less* than local apps by default. There are no special cases, and therefore no special-case bugs.

**Thesis 6 — Rayland doubles as crash recovery.**
Rayland clients attach through a proxy that holds enough state to detach and reattach (mosh semantics). A local loopback proxy therefore gives *local* apps opt-in survival across a compositor restart — the escape hatch Wayland famously lacks. This is also the development ergonomics story: restart the core a hundred times a day without losing your windows.

## 5. Non-goals

- **Not a desktop environment.** Parhelion is the core plus reference servers. Panels, launchers, settings are ordinary clients anyone can replace.
- **Not a wlroots or Smithay competitor.** We *consume* Smithay for protocol machinery; we do not aim to be a general-purpose compositor library.
- **Not an AAA remote-gaming system.** That is Rayland's video regime, an escape hatch, and explicitly out of scope here.
- **Not an X11 revival.** XWayland support is a compatibility server like any other, isolated accordingly.
- **Not a pixel-perfect clone of any existing WM's behavior.** Policy is a replaceable daemon; the reference policy daemon is deliberately modest.
- **No realtime guarantees over the network.** Inherited verbatim from Rayland's design: wire-induced latency is mitigated, never eliminated.

## 6. Success criteria (falsifiable)

1. **Boring-desktop parity:** on a laptop running terminals and browsers, idle power draw and input-to-photon latency within measurement noise of sway on the same hardware.
2. **Stall immunity:** a deliberately hostile extension or policy daemon (busy-loop, sleep(10), crash-loop, memory hog) cannot cause a single missed frame deadline in the core. This is a CI test, not a claim.
3. **Crash-only proof:** `kill -9` of every non-core process, one at a time, during an interactive session, with full recovery and no client death.
4. **Rayland indistinguishability:** a Rayland-remoted application is, to the core, an ordinary client; no code path in the core names Rayland.
5. **Regime collapse:** after any animation ends, the compositor returns to damage-tracked/plane-offloaded operation within ≤ 2 frames, verified by instrumentation.
6. **Restart survivability:** loopback-proxied clients survive a core restart with window state intact.

## 7. Naming

"Parhelion" — a *sundog*: a bright second sun that appears beside the real one, produced by refracted rays. The lineage nod (Sun Ray → Rayland → a companion that renders by rays) is intentional; the metaphor (a faithful second image of something whose light originates elsewhere) is on-point for the Rayland reference host. Alternatives considered: Sundog (friendlier, less unique), Analemma, Firmament, Penumbra. Final choice is ADR-0001; nothing below the repo name depends on it.

## 8. How this project is run

- **Documents are authoritative; code is downstream.** Claude Code tasks cite doc sections; contradictions discovered in implementation come back as design questions and amend the doc *before* the code.
- **Decision log** (`decisions/`): append-only, dated, one decision per entry — chose X, rejected Y, because Z, revisit-if W. Never edited after acceptance; superseded by later entries. Existence rule: any argument that happens twice must become an entry.
- **Diary** (`DIARY.md`): dated, informal, append-only; what was attempted, what surprised us, what to pick up next session. The diary is for narrative memory; the decision log is for settled reasoning.
- **Re-entrant documents:** every governing document begins with a re-entrancy header (status, date, kind, what it governs, companions) sufficient for a cold session — human or model — to orient without reading history.
- **Milestones are usable compositors.** Every milestone from M0 onward boots, composes, and is daily-drivable at its own level of ambition. There is no "big bang integration" phase.

*(These conventions inherit from the ENO project's practice; align details with ENO's actual formats when transcribing them into `decisions/0000-conventions.md`.)*
