# voplay Architecture

## Known Limitations

### 1. Scene2D / Scene3D Structural Similarity

The query methods (`FindByTag`, `ForEach`, `EntityCount`, `Entities`, `VisibleEntities`)
are structurally identical 5-10 line loops in both packages. This is **intentional and
acceptable** — the two scene types are genuinely different things with different entity
fields, physics systems, and rendering semantics.

Alternatives considered and rejected:
- **Generics**: `Scene[E]` would unify the iteration but adds language complexity the
  current Vo version doesn't have, and the methods are too simple to justify the overhead.
- **Embedded `EntityBase` struct**: Would deduplicate 4 shared fields (`Tag`, `Active`,
  `Visible`, `Data`) but forces verbose struct literal syntax
  (`EntityBase: voplay.EntityBase{Tag: "player"}` instead of `Tag: "player"`). Net
  ergonomic impact is negative.
- **Interface-based abstraction**: Adds indirection without proportional benefit at this
  scale (~50 lines of simple iteration code).

The duplication is manageable: changes to one scene's query methods are trivially applied
to the other. Both scenes use `entities []*Entity` as the internal field name for
consistency.

### 2. Game State Pattern

`GameCtx.Data any` has been **removed**. It encouraged a runtime-unsafe type-assertion
pattern that is strictly worse than the alternatives available to game code:

```vo
// Anti-pattern (removed):
s := g.Data.(*scene2d.Scene)

// Idiomatic: package-level variable (for named callback functions)
var s = scene2d.New()
func update(g *voplay.GameCtx, dt float64) { s.StepAndSyncPhysics(dt) }

// Idiomatic: closure capture (for inline function literals)
s := scene2d.New()
voplay.Run(voplay.Game{
    Update: func(g *voplay.GameCtx, dt float64) { s.StepAndSyncPhysics(dt) },
})
```

Games are single-instance applications that own their package namespace. Package-level
variables are the correct tool. `Data any` only makes sense in library-context code
(like `http.Request.WithContext`) where the library doesn't own the user's types.

### 3. Entity Componentization — RESOLVED

Physics runtime state (`bodyID`, `velocity`) moved from Entity into the Physics struct.
Entity now holds only: transform + identity/metadata + optional component pointers
(`*Sprite`, `*Physics`). Adding new component types (particles, audio emitters, etc.)
means adding a new pointer field — existing entities that don't use it pay zero cost.

`Velocity()` now panics if called on a non-physics entity (programming error, not a
runtime condition to handle gracefully).

### 4. Instance-Based Architecture — RESOLVED

All runtime state is now instance-scoped:
- `var gameCtx *GameCtx` removed from `game.vo`
- `var surfaceInitiated`, `var gameInitialized` removed from `host_vogui.vo`
- Lifecycle flags moved into `GameCtx` as unexported fields
- `runWeb`, `runNative`, `startGameLoop` take `*GameCtx` as parameter

Multiple `Game` instances can now run concurrently (each `Run` call creates its own
`GameCtx`). Studio embedding is architecturally possible.

### 5. Internal Types in `voplay/codec` — RESOLVED

`ByteWriter`, `ByteReader` moved to `voplay/codec` sub-package. Root package and
scene sub-packages import `codec` directly. Draw opcode constants (`opClear`,
`opDrawSprite`, etc.) are now unexported in `draw.vo` — sub-packages use `DrawCtx`
methods, not raw opcodes. The `voplay` namespace no longer exposes serialization
primitives to game code.
