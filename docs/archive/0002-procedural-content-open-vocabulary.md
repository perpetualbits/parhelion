> **Superseded in format, not content** — its decision now lives in the decision-log entry "2026-07-24 — Procedural content and vocabularies" (`../parhelion_decision_log.md`); reasoning moved to `../parhelion_desktop_dialect.md` and `../VISION.md`. Archived verbatim; do not edit.

# ADR-0002 — Procedural content: open vocabulary via shaders, fixed vocabularies only where compositor-owned

> **Status:** Accepted · **Date:** 2026-07-24 · **Kind:** decision record (append-only; supersede, never edit).
> **Context docs:** `VISION.md` §2 (ship intent), `CORE-BOUNDARY.md` I-12; Rayland design §6 (content-addressed asset residence).

## Decision

1. **Application procedural content travels as shaders + parameters.** Gradients, IFS/fractal systems, procedural textures, and any other algorithmic content cross the wire as SPIR-V (or command-stream-referenced pipelines) named by BLAKE3 content hash in the Rayland asset cache, plus per-frame uniform deltas. The shared "algorithm library on both systems" is not curated; it converges automatically from what applications actually ship, with the content hash as the universal name. Steady-state cost per procedural surface per frame: command reference + uniform delta.

2. **Fixed procedural vocabularies exist only in compositor-owned protocols**, where Parhelion controls both ends and versioning:
   - the **shape extension**: client-declared outline paths (Bézier segments / control points);
   - the **animation IR** executed by the core interpolation engine (curves, springs, timelines);
   - **decoration and cursor descriptions** (rounded-rect, gradient, shadow, vector cursor parameters).
   These are small, versioned, and deliberately closed; lock-in there is the spec, not a bug.

3. **Shape is declared, not extracted.** From one declared path the core derives, at declare-time: an inscribed (guaranteed-inside) polygon feeding occlusion culling, and a bounding (guaranteed-outside) polygon feeding damage and input hit-testing. Alpha-contour extraction from rendered buffers (marching squares + simplification) is retained **only** as a compatibility fallback for non-cooperating clients, and never drives the design.

## Rejected

**A curated cross-system library of drawing/procedural algorithms as the network vocabulary** (splines, gradient functions, IFS, etc. as fixed protocol primitives for application content). Rejected because fixed vocabularies for application rendering have a consistent historical failure mode — vocabulary lock-in: NeWS, Display PostScript, and X11's server-side drawing primitives all lost when applications needed effects outside the anticipated vocabulary and the pixel fallback became the norm. Any "few handfuls of algorithms" chosen now is the wrong few handfuls within years. The shader + content-hash path achieves the same "language over the network" goal with an open-ended vocabulary at equal or lower steady-state wire cost.

## Consequences

- The Rayland asset cache is load-bearing for procedural content, not just textures; cache persistence and hash-verification (Rayland doc §11, residence-oracle trust) rise in priority.
- The shape extension spec and animation IR spec (control-plane document) own their vocabularies and their versioning; proposals to add application-content primitives to compositor protocols are answered by this ADR.
- SPIR-V entering S remains hostile input: validation/sanitization in the sandboxed replay service per CORE-BOUNDARY I-8; this ADR adds no new trust.

## Revisit if

- A class of procedural content emerges that demonstrably cannot ride the shader path (e.g., host-side text shaping decisions) — handle as a compositor-owned vocabulary case, not by reopening the curated-library idea.
- SPIR-V is superseded as the portable GPU IR; the decision transfers to its successor unchanged in structure.
