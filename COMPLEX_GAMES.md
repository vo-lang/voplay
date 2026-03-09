# voplay for Complex Games

## Goal

This document defines the architectural direction for evolving `voplay` from a compact demo-friendly runtime into a foundation that can support larger and longer-lived games without losing its current simplicity.

The intent is not to turn `voplay` into a general-purpose engine. The intent is to harden the current design around the failure modes that appear once a game has:

- multiple gameplay states
- persistent world simulation
- larger entity counts
- real asset lifecycles
- camera behavior beyond simple follow
- stricter determinism requirements

## Current Strengths

The current architecture already has several correct decisions that should be preserved:

- `DrawCtx` is an immediate command encoder instead of a retained scene graph.
- `scene2d` and `scene3d` are separate packages instead of being forced into a weak abstraction.
- game-owned state lives in game code, not in a generic `any` bag on `GameCtx`.
- runtime state is instance-scoped, which keeps multi-instance hosting possible.
- entities use a pragmatic optional-component shape instead of premature ECS.

These are good foundations. The next step is to improve the runtime contract around them.

## Architectural Priorities

### Phase 1: Simulation Stability and Core Runtime

The runtime should prefer a correct long-term contract over source compatibility. `voplay` does not yet have an established game corpus depending on the current API, so architectural cleanup should happen now instead of being deferred behind compatibility layers.

#### 1. Fixed timestep simulation

`voplay` should support a fixed-step simulation loop. Physics, gameplay rules, and any logic that depends on stable stepping should run in fixed increments. Per-frame update should remain available for visual work and interpolation.

Target model:

- `State.FixedUpdate(g, fixedDt)` for simulation
- `State.Update(g, dt)` for per-frame logic
- `State.Draw(g)` for rendering
- `GameCtx.Alpha` for interpolation between fixed steps

`FixedUpdate` should be part of the required state contract, not an optional extension hook. A fixed-step runtime that only conditionally participates in simulation creates an unclear programming model.

This is the highest priority change because unstable frame-time driven simulation causes correctness problems across physics, timers, AI, and replayability.

#### 2. Resource lifecycle management

Complex games need grouped asset ownership, deduplicated loads, and explicit release points. Bare load/free handles are not enough once multiple states and scenes reuse resources.

Target model:

- keyed asset lookup
- deduplicated loads
- group-based ownership
- explicit release of groups

This should be implemented as an additive runtime service, not as hidden global state.

#### 3. Input action mapping

Raw key-string polling is too low-level for larger games. The runtime should provide an action layer on top of raw input.

Target model:

- bind one action to multiple keys
- per-action `down`, `pressed`, `released`
- simple digital axis helpers
- later extension point for gamepad input

### Phase 2: World Scaling

#### 4. Spatial queries that do not degrade linearly

`scene2d` currently uses linear scans for most entity queries. That is acceptable for small scenes but will become a structural bottleneck.

Target model:

- internal spatial hash or similar broad-phase structure
- accelerated radius and rectangle queries
- scene query semantics defined around the new runtime contract, not around preserving legacy surface shape

#### 5. Collision layers and masks

Games need first-class control over collision filtering.

Target model:

- layer bitmask on physics bodies
- mask bitmask on physics bodies
- direct mapping to backend physics filtering

#### 6. Camera behavior beyond follow

A serious 2D camera needs more than center-follow.

Target model:

- deadzone follow
- bounds clamping
- camera shake
- smoothed zoom changes

### Phase 3: Authoring Ergonomics

#### 7. Scheduler and tweening

Reusable timing primitives remove a large amount of boilerplate from gameplay code.

Target model:

- delayed callbacks
- repeating timers
- cancelation
- reusable tween primitives

#### 8. Animation state control

Linear frame playback is not enough for a character-heavy game.

Target model:

- named animation states
- explicit state changes
- transition conditions
- support for one-shot and looping clips

#### 9. Parent-child transforms

A light hierarchy is useful for equipment, attached FX, and composed UI-like in-world objects.

Target model:

- optional parent-child relation
- inherited transform
- no full retained scene graph

### Phase 4: Developer Tooling

#### 10. Debug rendering

A runtime for larger games should be able to render:

- collider bounds
- physics contacts
- debug labels
- camera bounds
- broad-phase cells

## Explicit Non-Goals

The following should not be introduced as part of this evolution plan:

- a mandatory ECS rewrite
- a retained-mode renderer
- a forced unification of `scene2d` and `scene3d`
- hidden global mutable runtime services
- defensive APIs that silently ignore invalid game logic

## Implementation Order

Recommended order:

1. fixed timestep runtime
2. input action mapping
3. asset manager
4. collision layers and masks
5. spatial indexing for `scene2d`
6. camera extensions
7. scheduler and tweening
8. animation controller
9. parent-child transforms
10. debug rendering

## First Implementation Slice

The first implementation slice starts with fixed timestep support because it changes the runtime contract in the most important way and should become the basis for all later gameplay-facing APIs.

The first slice should deliver:

- `Game.FixedStep`
- `GameCtx.FixedStep`
- `GameCtx.Alpha`
- required `State.FixedUpdate` contract
- fixed-step execution integrated into both web and native hosts
- tests for ordering and barrier behavior

Once that is in place, later systems can reliably build on top of a stable simulation clock.
