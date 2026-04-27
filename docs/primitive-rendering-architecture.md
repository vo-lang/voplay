# voplay primitive3d rendering architecture

## Goal

`primitive3d` is a relatively independent voplay module for rendering worlds built from a fixed vocabulary of primitive shapes and material presets.

When a game scene is mainly composed from `N` shape families and `M` material entries, renderer cost should be tied to visible shape/material groups, not object count:

```text
draw calls ~= render passes * visible(shape_id, material_id) groups
steady CPU sync ~= changed primitive ranges, not total primitive instances
```

This is the engine path for toy-scale racing scenery, procedural props, repeated barriers, rocks, trees, bridge blocks, signs, clouds, and primitive-composed vehicles. MarbleRush consumes this module; MarbleRush does not own it.

## Module Boundary

The public Vo module is:

```text
github.com/vo-lang/voplay/primitive3d
```

The module imports only root `github.com/vo-lang/voplay` types such as `Vec3`, `Quat`, `Color`, `AABB`, `TextureID`, and `MaterialDesc`.

`primitive3d` does not import `scene3d`, track, vehicle, terrain, physics, racing input, or MarbleRush code.

`scene3d` integrates it through a small adapter:

```vo
func (s *Scene) AddPrimitiveLayer(layer *primitive3d.Layer) primitive3d.LayerHandle
func (s *Scene) RemovePrimitiveLayer(handle primitive3d.LayerHandle)
func (s *Scene) PrimitiveStats() primitive3d.Stats
```

`scene3d.Scene.Draw` flushes attached primitive layers using the existing scene render id. `DrawScene3D(sceneID)` then renders both retained model entities and retained primitive layers for that scene.

Existing `scene3d` APIs remain responsible for camera, lights, shadows, fog, color grading, physics, animation, and complex entities.

## Existing Code Decisions

The current code already has these pieces:

- `resources.vo` exposes root primitive mesh creation hooks.
- `rust/src/model_loader.rs` caches primitive mesh models by shape key.
- `rust/src/pipeline3d.rs` already batches non-skinned repeated meshes with instance buffers.
- `rust/src/render_world.rs` owns retained 3D scene objects keyed by scene id and object id.
- `draw.vo` owns the binary draw stream opcodes and must remain the root package boundary.

Therefore the implementation decision is:

- Phase 1 reuses existing cached primitive meshes and model instancing for correctness.
- Phase 2 adds dedicated primitive retained storage and a dedicated primitive pipeline.
- The public module is `primitive3d`, not `scene3d/primitives`.
- `scene3d/primitives.vo` remains a compatibility wrapper for raw mesh creation, but new engine/game code should use `primitive3d`.

## Non-Goals

- No MarbleRush-specific vehicle, race, or track rule is encoded in `primitive3d`.
- No physics ownership in primitive instances. Physics stays in `scene3d`.
- No skeletal animation path. Animated characters and skinned models stay on `scene3d.Entity`.
- No generic transparent sorting in P0. Opaque primitives are the production path first.
- No debug overlay geometry inside production primitive layers.

## Public API

### Shape Registry

```vo
type ShapeID int

type ShapeKind int

const (
    ShapeRoundedBox ShapeKind = 1
    ShapeBox ShapeKind = 2
    ShapeSphere ShapeKind = 3
    ShapeCylinder ShapeKind = 4
    ShapeCone ShapeKind = 5
    ShapeCapsule ShapeKind = 6
    ShapePlane ShapeKind = 7
    ShapeWedge ShapeKind = 8
)

type ShapeDesc struct {
    Kind ShapeKind
    Segments int
    Radius float64
    BevelRadius float64
    Width float64
    Height float64
    Depth float64
    SubX int
    SubZ int
}

type ShapeRegistry struct { ... }

func NewShapeRegistry() *ShapeRegistry
func (r *ShapeRegistry) Shape(desc ShapeDesc) ShapeID
```

Rules:

- Equal `ShapeDesc` values return the same `ShapeID`.
- `RoundedBox` is the default toy hard-surface shape.
- `Box` exists for exact collision/debug/technical geometry, not for production toy art.
- Shape IDs are stable within a registry.
- The renderer, not game code, owns GPU mesh creation.

P0 shape set:

- `RoundedBox`
- `Box`
- `Sphere`
- `Cylinder`
- `Cone`
- `Capsule`
- `Plane`
- `Wedge`

P1 shape set:

- `Torus`
- `Arch`
- `CurbBlock`
- `RailSegment`
- `RoadPatch`

### Material Palette

```vo
type MaterialID int

type MaterialPreset int

const (
    MaterialToyPlastic MaterialPreset = 1
    MaterialRubber MaterialPreset = 2
    MaterialRoadAsphalt MaterialPreset = 3
    MaterialGrass MaterialPreset = 4
    MaterialStone MaterialPreset = 5
    MaterialWater MaterialPreset = 6
    MaterialGlow MaterialPreset = 7
    MaterialWood MaterialPreset = 8
)

type MaterialDesc struct {
    Preset MaterialPreset
    Base voplay.MaterialDesc
    Name string
}

type MaterialPalette struct { ... }

func NewMaterialPalette() *MaterialPalette
func (p *MaterialPalette) Material(desc MaterialDesc) MaterialID
```

Ownership decision:

- `MaterialPalette` is explicit and game-created.
- It belongs to `primitive3d`, not root `voplay` and not `scene3d`.
- It stores material entries and texture IDs.
- It does not own texture lifetime. Textures are still owned by `voplay.Assets` or manually freed by the caller.
- A `Layer` references one palette. Multiple layers may share a palette.

Material presets are semantic defaults. A toy plastic material is not just a color; it sets roughness, specular behavior, normal defaults, and shading mode.

Required P0 presets:

- toy plastic
- rubber
- road asphalt
- grass
- stone
- wood
- glow

P1 presets:

- water
- painted metal
- glass-like transparent material in a separate transparent layer

### Primitive Instance

```vo
type ObjectID int

type Instance struct {
    Shape ShapeID
    Material MaterialID
    Position voplay.Vec3
    Rotation voplay.Quat
    Scale voplay.Vec3
    Tint voplay.Color
    Flags InstanceFlags
    Object ObjectID
}
```

Rules:

- `Shape` and `Material` are required.
- `Tint` is a per-instance multiplier and must not create a new material batch.
- `Object` is optional but stable when picking/debugging is needed.
- Primitive object IDs live in the layer namespace and do not share the `scene3d.Entity.ID` namespace.

### Layer

```vo
type LayerKind int

const (
    StaticLayer LayerKind = 1
    DynamicLayer LayerKind = 2
    TransparentLayer LayerKind = 3
)

type LayerDesc struct {
    Kind LayerKind
    Shapes *ShapeRegistry
    Materials *MaterialPalette
    Chunking ChunkingDesc
    Name string
}

type Layer struct { ... }

func NewLayer(desc LayerDesc) *Layer
func (l *Layer) Add(instance Instance) ObjectID
func (l *Layer) Set(id ObjectID, instance Instance)
func (l *Layer) Remove(id ObjectID)
func (l *Layer) Clear()
func (l *Layer) Build()
func (l *Layer) Stats() Stats
```

Rules:

- `StaticLayer` is for scenery and uploads only dirty chunks/ranges after `Build`.
- `DynamicLayer` is for transforms that may change every frame.
- `TransparentLayer` is a separate P1 path. It is not mixed into opaque static batches.
- A layer is render data, not gameplay state.
- Games may keep their own composition structs and rebuild layers from them.

### Builder

`Builder` is the ergonomic authoring API above `Layer`. It keeps shape and material registries together with the layer, so primitive-composed scenery and vehicles can describe parts without scattering registry/palette plumbing through game code.

```vo
type BuilderDesc struct {
    Kind LayerKind
    Shapes *ShapeRegistry
    Materials *MaterialPalette
    Chunking ChunkingDesc
    Name string
}

type PartDesc struct {
    Shape ShapeDesc
    ShapeID ShapeID
    Material MaterialDesc
    MaterialID MaterialID
    Position voplay.Vec3
    Rotation voplay.Quat
    Scale voplay.Vec3
    Tint voplay.Color
    Flags InstanceFlags
    Object ObjectID
}

func NewBuilder(desc BuilderDesc) *Builder
func (b *Builder) Shape(desc ShapeDesc) ShapeID
func (b *Builder) Material(desc MaterialDesc) MaterialID
func (b *Builder) AddPart(desc PartDesc) ObjectID
func (b *Builder) AddInstance(instance Instance) ObjectID
func (b *Builder) Build() *Layer
func (b *Builder) Layer() *Layer
```

Rules:

- `AddPart` deduplicates equal shape and material descriptions through the builder-owned registries.
- Callers can pass explicit `ShapeID` and `MaterialID` when many parts share the same entries.
- `Build` returns the underlying layer after rebuilding static chunks.
- The builder remains in `primitive3d`; it does not depend on `scene3d` or MarbleRush.

## Scene Integration

`scene3d.Scene` owns the render scene id, camera, lights, shadows, and frame draw order. It is the correct integration point, but not the owner of primitive authoring logic.

Integration flow:

```text
primitive3d.Layer
  -> scene3d.Scene attachment
  -> Scene.Draw flushes layer changes into DrawCtx
  -> draw.vo primitive retained commands
  -> Rust RenderWorld primitive storage
  -> primitive pipeline draw during DrawScene3D(scene_id)
```

`scene3d.Scene.Close` destroys attached primitive layers from the renderer. The `primitive3d.Layer` object remains reusable by game code until the game discards it.

## Draw Stream Commands

`draw.vo` remains the only writer of binary opcodes. Because root `voplay` cannot import subpackages, low-level primitive stream methods use root/basic types and are called by `primitive3d`.

Primitive opcodes live after the existing retained scene range:

```text
0x34 Primitive3DUpsertInstance
0x35 Primitive3DDestroyInstance
0x36 Primitive3DClearLayer
0x37 Primitive3DDestroyLayer
0x38 Primitive3DReplaceChunk
0x39 Primitive3DReplaceChunkRefs
0x3A Primitive3DUpsertMaterials
0x3B Primitive3DUpsertShapes
0x3C Primitive3DReplaceChunkKeys
0x3D Primitive3DSetChunkVisible
```

`DrawScene3D(sceneID)` remains the draw trigger. There is no public `DrawPrimitiveLayer` command.

## Rust Renderer Modules

Add dedicated Rust modules:

```text
rust/src/primitive_scene.rs
rust/src/primitive_pipeline.rs
rust/src/primitive_shapes.rs
rust/src/primitive_material.rs
```

Responsibilities:

- `primitive_shapes.rs`: canonical mesh generation, rounded geometry, UVs, tangents, bounds.
- `primitive_material.rs`: material table, preset expansion, texture/sampler keys.
- `primitive_scene.rs`: retained primitive layers, chunks, explicit visibility, camera frustum culling, draw collection.
- `primitive_pipeline.rs`: GPU buffers, bind groups, instanced draw.

`render_world.rs` becomes:

```rust
pub struct RenderWorld {
    scenes: HashMap<u32, RenderScene>,
    primitive_scenes: PrimitiveRenderWorld,
}
```

Model entities and primitive layers share camera/lights/shadow state but keep separate retained storage.

## GPU Data

Instance data:

```rust
#[repr(C)]
struct PrimitiveInstanceGpu {
    model: [[f32; 4]; 4],
    normal_matrix: [[f32; 4]; 4],
    tint: [f32; 4],
    object_id: u32,
    flags: u32,
    _pad: [u32; 2],
}
```

Material data:

```rust
#[repr(C)]
struct PrimitiveMaterialGpu {
    base_color: [f32; 4],
    emissive_color: [f32; 4],
    params: [f32; 4], // roughness, metallic, normal_scale, uv_scale
    texture_ids: [u32; 4],
    flags: [u32; 4],
}
```

Material data lives in a material table/bind group. Per-instance data does not duplicate material parameters.

## Batching Rules

One draw group is defined by:

```text
render pass
shape id
material id
pipeline id
texture/sampler set
shadow/depth/transparent class
```

These do not break a batch:

- transform
- normal matrix
- tint
- object id
- visibility inside a visible chunk

Opaque batches draw before transparent batches. Transparent primitives are P1 and live in `TransparentLayer`.

## Chunking and Culling

Static layers are chunked. Dynamic layers are range-updated.

Chunking decision:

- Default static chunking is a 3D grid using configurable cell size.
- Track-like games may choose distance-based chunking using authoring tools.
- The Vo layer builds chunk membership from authoring data.
- Rust `PrimitiveRenderWorld` rebuilds chunk bounds from retained primitive instances and culls chunks against the current `Camera3DUniform` during draw collection.
- Non-chunk primitive instances are also culled in Rust during draw collection.

No steady-state frame may scan all static instances in Vo just to discover camera visibility. Camera motion must be handled by renderer-side retained data.

## Physics Decision

`primitive3d` is render-only.

Physics stays in `scene3d`:

- track collision meshes
- terrain heightfields
- vehicle bodies
- entity colliders
- raycasts

If a primitive-composed object needs physics, game code creates a `scene3d.Entity` or track/terrain physics body as the authoritative physical object, then updates primitive visuals from that object.

This prevents primitive rendering from becoming another entity system.

## Picking Decision

Picking uses `LayerHandle + ObjectID`.

Primitive picking returns:

```vo
type PickHit struct {
    Layer LayerHandle
    Object primitive3d.ObjectID
    Position voplay.Vec3
    Normal voplay.Vec3
    Distance float64
}
```

`scene3d` may expose a combined pick API later, but primitive IDs do not share `scene3d.Entity.ID`.

## Visual Quality Contract

The module exists because fixed primitives can look high quality when geometry and material response are authored correctly.

Required:

- `RoundedBox` is the default production block.
- Production hard-surface primitives include bevels or bevel normals.
- Meshes have correct normals, tangents, UVs, and bounds.
- Toy plastic, rubber, asphalt, grass, stone, wood, glow, and water presets are tuned materials.
- Same-material instances can use tint/noise variation without splitting batches.
- Shadow, ambient ground, tone mapping, roughness, and normal maps are part of the expected path.

Concept-art quality comes from:

- beveled silhouettes
- clean shape language
- contact shadows
- different roughness by material
- controlled color palette
- repeated primitives with small per-instance variation

## MarbleRush Usage

MarbleRush should express repeated scenery and simple vehicle parts as primitive composition data.

Use primitive layers for:

- red/white curbs
- tire barriers
- fences
- toy trees
- bushes
- rocks
- bridge blocks
- cliff blocks
- clouds
- road signs
- simple towers/buildings
- non-hero kart panels and wheels

Keep GLB/model entities for:

- hero meshes
- animated characters
- unique sculpted assets
- complex tunnel interiors
- content where authored mesh detail matters more than batching

Wheel rotation and steering are dynamic primitive transform updates when the kart is primitive-composed.

## Performance Contract

P0 acceptance:

- 10000 static opaque primitives across a small number of shape/material pairs do not produce object-count draw calls.
- Stable static layers do not upload full instance buffers every frame.
- 1000 dynamic primitives can update transforms without rebuilding static scenery.
- The renderer reports primitive groups, instance counts, chunks, upload bytes, and draw calls.

Stats:

```vo
type Stats struct {
    TotalInstances int
    VisibleInstances int
    TotalChunks int
    VisibleChunks int
    DirtyRanges int
    ShapeMaterialGroups int
    DrawCalls int
    UploadBytes int
}
```

`scene3d.SceneRenderStats` may include aggregate primitive stats, but detailed stats live in `primitive3d.Stats`.

## Implementation Phases

### Phase 0: Module Contract

- Add `primitive3d` package.
- Define shape registry, material palette, layer, instance, stats, and handles.
- Add tests for registry caching, material ID reuse, static/dynamic dirty behavior, and stats.
- Add `scene3d` attachment adapter.

### Phase 1: Compatibility Renderer Path

- `primitive3d` uses existing cached primitive model generation internally.
- Attached layers flush to retained model objects or a compact interim representation.
- Renderer uses existing `Pipeline3D` instancing.
- This validates API and content workflows without committing to final GPU storage.

Phase 1 API must already match the final module design. Temporary internals must not leak into public types.

Current status: complete for the API and retained-scene integration. Rounded box, cone, and wedge now have production mesh generation hooks instead of being aliases for old generic primitives.

### Phase 2: Dedicated Primitive Renderer

- Add primitive retained commands to `draw.vo` and `stream.rs`.
- Add `PrimitiveRenderWorld` beside retained model objects.
- Static layer buffers upload only dirty chunks/ranges.
- Dynamic layer uses frame/ring buffer updates.
- Add `primitive_pipeline.rs` with shape/material grouped instanced drawing.
- `DrawScene3D(sceneID)` draws model entities, primitive opaque layers, terrain, decals/particles, then transparent primitive layers.

Current status: retained primitive commands are implemented through `PrimitiveRenderWorld`. Static chunks are sent with `Primitive3DReplaceChunkKeys`, so first upload and chunk refresh are one command per content chunk and each instance references layer-local shape and material ids. `Primitive3DSetChunkVisible` remains as an explicit retained-visibility control, not as the normal camera-culling mechanism. `Primitive3DUpsertShapes` and `Primitive3DUpsertMaterials` upload the small layer tables only when their revisions change. Primitive draws now split away from `ModelDraw` and render through `rust/src/primitive_pipeline.rs`, which owns its own instance buffer, bind groups, batch keys, and instanced draw loop. Chunk replacement also builds resident GPU chunk batches in the primitive pipeline; steady static frames render chunk references from retained Rust data without rebuilding those chunk instance buffers. Static scene flushing uses `Layer.DrainStaticChanges` to avoid constructing a whole-layer `Upserts` slice, caches layer shape/material group counts after chunk builds, and streams chunk-key instances directly into `DrawCtx` without allocating a per-chunk draw slice. Rust draw collection receives the active `Camera3DUniform` from `renderer.rs` and culls retained static chunks and non-chunk primitive instances before submitting draw groups.

### Phase 3: Chunking and Tooling

- Add grid chunk builder.
- Add track-distance chunk builder for racing scenes.
- Add asset/tool format for baked primitive layers.
- Add debug/stats visualization in voplay demos or Studio.

Current status: grid chunking is implemented in `primitive3d.Layer`, while camera culling is renderer-owned in Rust. Static layers cache chunk metadata for content upload only; if only the camera changes, the Vo adapter emits no primitive visibility commands and the Rust retained world culls chunks from its stored bounds. `primitive3d.NewStressFixture` is the shared fixture for both the draw-stream benchmark and the in-app renderer stress scene. `tools/primitive_stress.vo` measures first-upload, steady-frame, and camera-moved stream cost without a browser. `examples/primitive_stress` is the canonical Studio runner entry for real WebGPU validation; it starts at 10000 primitives, targets 120 fps, orbits the camera, and shows instance/chunk/group/upsert/byte counters in the HUD. Track-distance chunking and baked primitive asset tooling are still future work.

### Phase 4: Advanced Rendering

- Add transparent primitive layer.
- Add water material path.
- Add indirect or multi-draw backend where supported.
- Add primitive picking and object-id debug view.

## Test Requirements

Vo tests:

- equal `ShapeDesc` returns equal `ShapeID`.
- different shape params return different `ShapeID`.
- material palette reuses identical material entries.
- static layer steady draw has no full upload.
- dynamic layer transform update marks only changed ranges.
- scene adapter destroys renderer layers on `Scene.Close`.

Rust tests:

- primitive draw key grouping is stable.
- instance buffer layout matches shader layout.
- material table fallback is deterministic.
- Rust chunk culling excludes off-camera chunks before instance grouping.
- Rust object culling excludes off-camera non-chunk primitive instances before transient draw grouping.

Integration tests:

- a voplay demo renders 10000 primitive instances with a small number of draw groups.
- MarbleRush migrates repeated scenery to `primitive3d`.
- stats prove object count and draw call count are decoupled.
