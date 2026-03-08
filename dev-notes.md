# voplay Sub-Package Refactor Notes

## Summary

Unified 2D/3D scene management by moving scene types into sub-packages:
- `voplay/scene2d` — 2D scene, entity, camera, physics, sprite, tilemap, draw helpers
- `voplay/scene3d` — 3D scene, entity, camera, physics, lights, draw helpers

Root `voplay` package retains core primitives: math types, draw command encoder, input, audio, game loop.

## Architecture

```
voplay/                     (root package)
├── game.vo                 GameCtx, Game, Run(), texture/font/model/audio APIs
├── draw.vo                 DrawCtx — low-level binary command encoder (raw params only)
├── input.vo                InputState — keyboard/pointer/scroll
├── math.vo                 Vec2, Vec3, Quat, Rect
├── color.vo                Color, predefined colors
├── audio.vo                Audio helpers
├── host_vogui.vo           Game loop (web + native)
├── codec/                  (sub-package: github.com/vo-lang/voplay/codec)
│   └── codec.vo            ByteWriter, ByteReader — internal serialization
├── scene2d/                (sub-package: github.com/vo-lang/voplay/scene2d)
│   ├── scene.vo            Scene, Entity, Camera + scene management
│   ├── physics.vo          Physics, Collider, BodyType, Contact, RayCastHit + externs
│   ├── sprite.vo           Sprite, SpriteSheet, Animation
│   ├── tilemap.vo          Tilemap, TileSet, TileLayer
│   └── draw.vo             DrawSprite, DrawTilemap, DrawScene (convenience)
└── scene3d/                (sub-package: github.com/vo-lang/voplay/scene3d)
    ├── scene.vo            Scene, Entity, Camera, Light + scene management
    ├── physics.vo          Physics, Collider, BodyType, Contact, RayCastHit + externs
    └── draw.vo             SetLights, DrawScene (convenience)
```

## Key Design Decisions

1. **No circular deps**: Root draw.vo uses only root types (raw floats, Vec3, Quat, Color, TextureID).
   Sub-packages import root and provide convenience wrappers (DrawSprite, DrawScene).

2. **Unexported opcodes, codec sub-package**: Draw opcodes are unexported in draw.vo.
   ByteWriter/ByteReader live in `voplay/codec`, imported by root and scene sub-packages.
   Game code never sees serialization primitives.

3. **Type renaming**: Within sub-packages, types drop dimension suffix:
   - `Scene2D` → `scene2d.Scene`, `Entity2D` → `scene2d.Entity`, `Camera2D` → `scene2d.Camera`
   - `Scene3D` → `scene3d.Scene`, `Node3D` → `scene3d.Entity`, `Camera3D` → `scene3d.Camera`
   - Both scenes use `entities []*Entity` as the internal storage field name.

4. **User-managed physics**: GameCtx no longer auto-steps physics or animations.
   Users call `scene.StepAndSyncPhysics(dt)` and `scene.UpdateAnimations(dt)` explicitly.

5. **Rust externs**: Physics externs moved from `"voplay"` to `"voplay/scene2d"` and `"voplay/scene3d"`.
   3D externs renamed: `physics3dInit` → `physicsInit` (in scene3d package scope).

## User-Facing API

```vo
import "github.com/vo-lang/voplay"
import "github.com/vo-lang/voplay/scene2d"

// Package-level state: idiomatic for game code.
var s = scene2d.New()

func init(g *voplay.GameCtx) {
    s.Spawn(scene2d.Entity{
        X: 100, Y: 200, W: 32, H: 32,
        Sprite: scene2d.NewSprite(tex, 32, 32),
        Physics: &scene2d.Physics{
            Type: scene2d.Dynamic,
            Collider: scene2d.Box(16, 16),
        },
    })
}

func update(g *voplay.GameCtx, dt float64) {
    s.StepAndSyncPhysics(dt)
    s.UpdateAnimations(dt)
}

func draw(g *voplay.GameCtx) {
    g.Draw.Clear(0.1, 0.1, 0.2, 1)
    scene2d.DrawScene(g.Draw, s)
}
```

## Verification

- `cargo check` on voplay/rust — **PASSED** (Rust externs compile with new sub-package paths)
- `vo check` on voplay root — **PASSED** (root package type-checks cleanly)
- Sub-packages type-check when imported by consumer code (on-demand via ProjectImporter)

## Files Changed

### New files
- `scene2d/scene.vo`, `scene2d/physics.vo`, `scene2d/sprite.vo`, `scene2d/tilemap.vo`, `scene2d/draw.vo`
- `scene3d/scene.vo`, `scene3d/physics.vo`, `scene3d/draw.vo`

### Modified files
- `game.vo` — removed Scene2D/Scene3D from GameCtx, removed physics externs
- `draw.vo` — raw-param APIs, removed scene-aware methods, added W() + exported opcodes
- `input.vo` — WorldPointerPos takes raw camera params
- `host_vogui.vo` — removed auto physics/animation stepping
- `bytes.vo` — fixed uint64→int cast for Float64frombits
- `vo.mod` — updated files() list
- `rust/src/externs.rs` — updated #[vo_fn] package paths for physics externs

### Deleted files
- `scene2d.vo` (replaced by scene2d/ directory)
- `scene3d.vo` (replaced by scene3d/ directory)

### Upstream (volang)
- `vo-ffi-macro/src/resolve.rs` — sub-package path resolution for #[vo_fn] macro
