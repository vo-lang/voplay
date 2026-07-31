# Voplay Runtime Memory Contract

Status: Accepted
Date: 2026-07-30

Voplay uses Volang's per-Island managed-memory contract and adds bounded,
domain-owned buffers for simulation and presentation work. The canonical
language/runtime contract is maintained in the Volang repository at
`docs/game-memory-architecture.md` and `lang/docs/spec/runtime-memory.md`.

## Host profile

A low-jitter Voplay host should:

1. create each Volang Island with an explicit managed reserve and hard limit;
2. complete WebAssembly admission before guest execution;
3. warm the scene and domain pools;
4. disable managed growth for the steady-state interval when admitted capacity
   is sufficient;
5. disable managed allocation only around callbacks that have a strict
   allocation-free contract;
6. run bounded GC work at scheduler or frame boundaries;
7. observe managed, runtime-backing, external-provider, fragmentation, reclaim,
   and platform allocator telemetry.

No-growth covers Volang managed heap segments. GPU, audio, renderer, provider,
Rust protocol metadata, and product state need independent budgets.

## Stage buffers

`GameEngine` owns one `GameStageBuffers` instance and one cached system list per
stage. The following vectors are created from `GameEngineConfig` capacities and
reused with `clear`:

- render operations;
- frame transients;
- presentation-state operations;
- world commands;
- simulation resource operations;
- simulation events;
- logic-I/O commands;
- control transactions.

Every public emission API checks its configured logical capacity before push.
A capacity violation returns `GameEngineError::StageOutputCapacity`.

Endpoint events move into simulation state with append/swap-style reuse, so
both source and destination vectors retain reusable capacity. A stage without
resource operations skips the simulation resource-map transaction clone.
World and simulation transactions can still allocate when the product makes a
real state change that exceeds previously admitted domain capacity.

`GameEngineOwnerSnapshot` publishes:

- `stage_buffer_reserved_slots`, the sum of current vector capacities;
- `stage_buffer_peak_used_slots`, the largest combined stage-output length
  observed after systems finish.

These are logical element slots across heterogeneous vectors. They are useful
for headroom and regression tracking and do not represent exact bytes.

## Verification

The owning regression test is
`game_stage_buffers_are_reserved_once_and_reused_across_ticks`. It verifies
that repeated ticks preserve every stage-vector capacity and that owner
telemetry reports both reserve and observed use.

Strict product certification should also verify:

- Volang managed heap growth count remains unchanged in steady state;
- platform allocator calls remain unchanged in the selected callback;
- `stage_buffer_peak_used_slots` stays below the admitted reserve;
- provider and GPU/audio memory remain inside their own limits;
- GC step wall-time distribution meets the target device budget.
