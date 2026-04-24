# voplay retained renderer architecture

## Goal

voplay keeps the Vo game API and the native/web renderer contract stable while moving the 3D renderer from per-frame model commands to a retained render scene.

The public shape stays:

- `voplay.Game` drives the frame loop.
- `scene3d.Scene` owns camera, lights, physics, animation, and entities.
- Studio only provides the canvas, island transport, and renderer extension host.

The internal 3D path becomes:

```text
scene3d.Scene
  -> dirty entity/state scan
  -> retained scene commands
  -> Rust RenderWorld
  -> batch/instance render lists
  -> wgpu pipelines
```

## Current bottleneck

`scene3d.Scene.Draw` currently expands every visible entity into `DrawModel` every frame. `Renderer.submit_frame` decodes that full stream, rebuilds every model matrix, and `Pipeline3D.draw_models` uploads one uniform slot and issues one indexed draw per mesh instance.

That means a scene with many repeated building blocks has CPU and command-encoder cost proportional to object count every frame, even when the world is static.

## Target boundaries

### Vo API

The user-facing scene API remains retained. Entity creation, transform changes, visibility, tint, model selection, physics sync, and animation are still expressed through `scene3d.Scene` and `scene3d.Entity`.

### Draw stream

2D, text, sprites, billboards, and direct draw APIs can remain frame commands. The 3D scene path uses retained commands:

- upsert object
- destroy object
- clear scene
- draw scene

The initial retained command can carry a full object payload on creation or change. Later revisions can split transform, material, and visibility into smaller commands without changing the renderer boundary.

### Rust renderer

Rust owns a `RenderWorld` keyed by scene id and object id. The draw stream mutates this world, then asks it to draw a scene with the current camera, light, fog, shadow, and skybox state.

The renderer builds render lists from retained objects and groups static meshes by model, mesh, material, texture, and pipeline.

### Studio

Studio remains framework-neutral. It does not know about game objects, MarbleRush, voplay scene internals, or asset-specific behavior.

## Rendering path

```text
Scene.Draw
  SetCamera3D / SetLights3D / SetFog3D / SetShadow3D / DrawSkybox
  Flush retained object changes
  DrawScene3D(scene_id)
  Draw emitters and HUD commands

Renderer.submit_frame
  Decode 2D/direct commands
  Apply retained 3D commands to RenderWorld
  Collect retained scene draws
  Render shadow pass if enabled
  Render 3D batches
  Render 2D draw list
```

## Performance direction

The retained path removes full-scene serialization from steady-state frames. Instancing removes the one-object-one-draw-call pattern for repeated static meshes. Culling and material sorting then become local renderer concerns instead of Vo API concerns.

`UncappedFrameRate` is only a measurement and iteration tool. The actual performance path is retained scene state plus GPU-side batching.
