# voplay kart racing engine development plan

## Goal

Build the generic voplay engine capabilities required for MarbleRush to become a production-quality kart racing game.

This document intentionally defines engine requirements, not MarbleRush game rules. voplay should know about racing tracks, vehicles, terrain, materials, asset loading, diagnostics, and runtime performance. MarbleRush should decide lap count, item behavior, boost balance, UI, characters, and level themes.

## Boundaries

### Engine owns

- Versioned racing track asset format.
- Track loading from filesystem and `vopack`.
- Track validation and spatial queries.
- Continuous terrain, track visual mesh, track collision mesh, and surface metadata.
- Generic raycast vehicle and kart controller primitives.
- Material, lighting, and debug features needed by 3D racing games.
- Asset baking and packaging contracts that are reusable by other games.

### Game owns

- Race rules and win conditions.
- Item systems, scoring, economy, and progression.
- Exact boost values, grass penalties, obstacle behavior, and character tuning.
- HUD, menus, theme naming, and level-specific scripting.
- Any IP-specific art direction.

## Definition of Done

Every engine requirement is complete only when all of these are true:

- The public API is documented in code comments or a `docs/` file.
- Native and web runner behavior match for the required path.
- A focused automated test covers the non-rendering behavior.
- A MarbleRush or voplay demo exercises the feature end to end.
- Invalid input produces deterministic errors, not silent fallback or unrelated panics.
- Resource ownership is clear: assets loaded through `Assets` are group-managed, manually loaded assets are explicitly released, and scene-owned resources are cleaned up by `Scene.Close`.

## Priority Levels

- P0: Required before MarbleRush can become a real kart racing game.
- P1: Required before the game can feel polished and content-scalable.
- P2: Required for long-term production quality, larger content, or advanced modes.

## P0 Requirements

Track data contract notes live in `docs/track-asset-v1.md`.

### TRK-001: Versioned TrackAsset Schema

voplay must define a first-class `TrackAsset` schema separate from the current generic map schema.

Required fields:

- `version`
- `name`
- `closedLoop`
- terrain references
- visual mesh references
- collision mesh references
- centerline control points
- per-segment width
- per-segment banking
- per-segment elevation hint or sampled height binding
- surface regions
- checkpoints or gates
- spawn poses
- respawn poses
- trigger regions
- authoring metadata

Acceptance criteria:

- `scene3d.LoadTrackAsset(path)` loads a minimal valid track from the filesystem.
- `scene3d.LoadTrackWithAssets(scene, assets, path)` loads the same track from a mounted `vopack`.
- The schema rejects unsupported versions with an error containing the unsupported version number.
- The schema rejects missing required resources with an error containing the missing asset path.
- The schema rejects duplicate track element names with an error containing the duplicate name.
- The schema rejects gates whose order is not strictly increasing along the track.
- The schema rejects a closed-loop track whose centerline endpoints are not continuous within a documented tolerance.
- Unit tests cover one valid minimal track and at least six invalid tracks.
- The schema documentation defines coordinate system, units, forward direction, yaw convention, and angle units.

Current coverage:

- `scene3d.TrackAsset` is the versioned runtime track schema and includes terrain, terrain chunks, visual/collision meshes, centerline width/banking/jump hints, surfaces, gates, spawns, respawns, triggers, racing lines, and authoring metadata.
- `scene3d.LoadTrackAsset`, `LoadTrackWithAssets`, `ValidateTrackAssetFiles`, and `ValidateTrackAssetResources` cover filesystem and mounted `vopack` loading paths.
- Validation rejects unsupported versions, missing terrain/mesh resources, duplicate names, unordered gates, discontinuous closed loops, invalid respawns, invalid surface ranges, non-finite values, and invalid racing lines with deterministic `TrackIssue` records.
- `docs/track-asset-v1.md` defines units, axes, yaw convention, forward direction, lateral sign, and closed-loop tolerance.
- `tests/main.vo::testScene3DTrackValidationAndQueries` covers the in-memory valid track plus more than six invalid schema cases.
- `tests/main.vo::testScene3DTrackAssetFilesystemLoading` covers direct filesystem `LoadTrackAsset` success plus unsupported version, missing resource, duplicate mesh name, and gate-order errors.

### TRK-002: Track Runtime Object

voplay must expose a runtime `Track` object created from `TrackAsset`.

Required API:

- `Track.Asset() *TrackAsset`
- `Track.Length() float64`
- `Track.ClosestPoint(pos voplay.Vec3) TrackPoint`
- `Track.PoseAt(distance float64) TrackPose`
- `Track.DistanceAlongTrack(pos voplay.Vec3) float64`
- `Track.SurfaceAt(pos voplay.Vec3) TrackSurfaceHit`
- `Track.IsOnRoad(pos voplay.Vec3) bool`
- `Track.NextGate(index int) TrackGate`
- `Track.RespawnPoseNear(pos voplay.Vec3) TrackPose`

Acceptance criteria:

- On a generated oval test track, `PoseAt(0)` and `PoseAt(Length())` match for closed loops within position tolerance `0.05`.
- `ClosestPoint` returns lateral distance with correct left/right sign on both straights and turns.
- `DistanceAlongTrack` is monotonic for samples walking forward along the centerline.
- `SurfaceAt` returns the highest-priority overlapping surface when multiple regions overlap.
- `IsOnRoad` returns false outside road width and true inside road width for sampled points.
- `RespawnPoseNear` never returns a pose outside the road boundary on the reference track.
- A benchmark or performance test records the cost of 10,000 `ClosestPoint` queries and fails only on correctness, not on a hard time budget.

Current coverage:

- `scene3d.Track` exposes the required API: `Asset`, `Length`, `ClosestPoint`, `PoseAt`, `DistanceAlongTrack`, `SurfaceAt`, `IsOnRoad`, `NextGate`, `RespawnPoseNear`, and racing-line pose lookup.
- `tests/main.vo::testScene3DTrackValidationAndQueries` covers closed-loop wrap, closest point distance and lateral sign, monotonic distance samples, overlapping surface priority, road/offroad checks, respawn pose selection, racing-line interpolation, missing line errors, and a 10,000-query correctness loop.
- MarbleRush consumes the same runtime object in `map_demo.vo` for spawn, gate placement, respawn placement, kart controller binding, and debug overlay.

### TRK-003: Track Validation Report

voplay must provide a validation API that can be used by tools and games before spawning a track.

Required API:

- `ValidateTrackAsset(asset *TrackAsset) []TrackIssue`
- `TrackIssue.Code`
- `TrackIssue.Severity`
- `TrackIssue.Path`
- `TrackIssue.Message`

Acceptance criteria:

- Validation returns structured issues, not only formatted strings.
- Validation distinguishes errors from warnings.
- Validation catches missing resources, duplicate names, invalid widths, invalid gates, invalid respawn poses, invalid surface names, and non-finite numeric values.
- A track with validation errors is not spawned by `LoadTrack`.
- A track with warnings can be spawned.
- The validation test suite includes at least one warning-only track and one error track.

Current coverage:

- `scene3d.ValidateTrackAsset` returns structured `TrackIssue` values with `Code`, `Severity`, `Path`, and `Message`.
- Validation distinguishes `TrackIssueError` from `TrackIssueWarning`; `scene3d.NewTrack`, `LoadTrackAsset`, and `LoadTrackWithAssets` block error tracks but allow warning-only tracks.
- Filesystem and mounted-pack resource validators return the same structured issue type.
- `tests/main.vo::testScene3DTrackValidationAndQueries` covers warning-only spawn/respawn omissions and multiple error tracks, including invalid widths, gates, resources, respawns, duplicate names, and non-finite positions.

### TRK-004: Track Spawning

voplay must spawn terrain, visual track mesh, collision mesh, gates, and optional debug helpers from a `Track`.

Acceptance criteria:

- A reference track loads and spawns from filesystem assets.
- The same reference track loads and spawns from a mounted `vopack`.
- Visual mesh and collision mesh can be different assets.
- Collision mesh may be hidden while visual mesh remains visible.
- If one spawn step fails, all previously spawned entities and manually loaded resources are released.
- A `TrackSpawnResult.Destroy(scene)` call removes every spawned entity and is idempotent.

Current coverage:

- `scene3d.SpawnTrackWithAssets` spawns terrain, terrain chunks, visual meshes, and hidden collision meshes from a `Track`.
- `scene3d.LoadTrack` and `LoadTrackWithAssets` create and spawn the runtime track from filesystem or mounted `vopack` assets.
- `TrackSpawnResult.Destroy` removes spawned entities, releases manually loaded textures, and is idempotent.
- `tests/main.vo::testScene3DTrackSpawnAndTraversability` covers filesystem-style spawning, mounted `vopack` loading, separate visual and collision mesh entities, hidden collision meshes, lifecycle cleanup, asset-group ownership, and rollback on missing packed resources.
- `tests/main.vo::testScene3DTrackTerrainChunks` covers multi-chunk terrain spawning and cleanup.

### TRK-005: Traversability Checks

voplay must provide tooling to verify that a track is physically drivable.

Required checks:

- Centerline has no discontinuity above threshold.
- Road boundary is continuous.
- Raycast to road or terrain succeeds at sampled centerline points.
- Height delta between adjacent samples stays below threshold unless the segment is marked as jump or ramp.
- Gate order is reachable along the track.

Acceptance criteria:

- The validator samples at least every 1 meter on the reference track.
- The validator reports the exact sample distance and world position for failures.
- The validator catches an intentionally broken track with a gap.
- MarbleRush can run the validator on its demo track without errors.

Current coverage:

- `scene3d.CheckTrackTraversability` samples the track by configurable meter step and reports structured issues with exact distance/path and world position in the message.
- Checks cover long centerline segments, centerline offroad samples, height deltas, road boundary discontinuities, height-probe misses/deltas, closed-loop continuity, and gate reachability.
- `tests/main.vo::testScene3DTrackSpawnAndTraversability` covers valid flat tracks plus intentionally broken height, boundary, offroad-centerline, unreachable-gate, missing-height-probe, and height-probe-delta cases.
- MarbleRush `tools/validate_track.vo` and `tools/pack_assets.vo` run the same traversability validator on `assets/maps/demo_track/track.json`; the current demo track validates with `SampleStep: 1.0` and no errors.

### VEH-001: Raycast Vehicle Telemetry

The current raycast vehicle wrapper must expose enough telemetry for tuning and debugging.

Required per-wheel telemetry:

- center
- contact point
- contact normal
- suspension length
- suspension compression ratio
- steering angle
- wheel rotation
- grounded state
- normal load, if available from Rapier or approximated consistently
- longitudinal slip, if available or approximated
- lateral slip, if available or approximated
- surface id under the wheel

Acceptance criteria:

- `Vehicle.WheelState(i)` returns stable data for all configured wheels after physics sync.
- Wheel telemetry includes surface id when the wheel contacts a `Track` surface.
- Telemetry never returns NaN or Inf on the reference track for a 60 second simulation.
- Native and web runners report the same wheel count and contact state on the same static test scene.
- Tests cover wheel state decoding, missing vehicle state, and vehicle destruction.

Current coverage:

- `tests/main.vo::testScene3DVehicleTelemetryAndKartController` covers wheel decoding, capped malformed backend state, surface id, normal load, longitudinal slip, lateral slip, missing backend state, and vehicle destruction.
- `tests/main.vo::testScene3DKartControllerSixtySecondStability` covers a deterministic 60 second controller/telemetry stability run.
- `tools/vehicle_telemetry_parity.vo` is the shared native/web parity probe. It currently reports `wheel_count=4` and `contacts=4` on both native and Studio/web wasm.
- `tools/check_vehicle_telemetry_parity.mjs` runs the native probe, executes the same probe through Studio/web wasm with voplay loaded as a local workspace dependency, and fails if wheel count or contact count diverge.

### VEH-002: KartController

voplay must provide a high-level kart controller built on top of raycast vehicles.

Required inputs:

- throttle
- brake or reverse
- steering
- drift
- boost
- reset

Required behavior:

- speed-limited acceleration
- reverse behavior
- braking behavior
- steering curve by speed
- traction assist
- anti-roll force
- downforce
- surface friction modifier
- drift entry, hold, and exit
- mini-turbo charge output
- boost force

Acceptance criteria:

- Steering left turns the kart left in both physics and wheel visual telemetry.
- Steering right turns the kart right in both physics and wheel visual telemetry.
- The kart can drive from grass back onto road on the reference track without manual respawn.
- On road, full throttle reaches configured max speed within documented tolerance.
- On grass, full throttle reaches a lower speed than road using the configured surface modifier.
- Drift can be entered only above configured minimum speed.
- Releasing drift emits a mini-turbo event only when charge passes the configured threshold.
- Boost increases speed or acceleration for the configured duration.
- A 60 second unattended simulation on the reference track has no NaN, no Inf, no exploding transform, and no unrecovered upside-down state.

Current coverage:

- `tests/main.vo::testScene3DKartControllerAcceptance` covers road/offroad scaling, drift threshold, turbo release, boost force, fall/upside-down/stuck respawn requests, and respawn state clearing.
- `tests/main.vo::testScene3DKartControllerSixtySecondStability` covers the 60 second unattended stability criterion against a deterministic reference-track harness.

### VEH-003: Vehicle Recovery and Respawn

voplay must provide generic stuck detection and respawn helpers for vehicles on tracks.

Acceptance criteria:

- A vehicle below a configured world Y threshold can be reset to the nearest valid respawn pose.
- A vehicle upside down for longer than a configured duration can be reset.
- A vehicle with near-zero speed and high throttle for longer than a configured duration can be marked stuck.
- Respawn clears linear velocity, angular velocity, steering state, drift state, and boost force.
- Respawn pose is aligned to the track forward direction.

Current coverage:

- `scene3d.VehicleRecovery` is the generic recovery helper for track-bound vehicles. It detects fall-below-world, upside-down duration, low-speed high-throttle stuck state, and explicit reset input.
- `VehicleRecovery.Respawn` and `RespawnNear` reset the underlying vehicle to a `TrackPose`, aligned to track forward direction.
- `Vehicle.SetPose` clears speed, steering, wheel spin, drift state, linear velocity, angular velocity, and raycast wheel controls.
- 3D physics body state now carries angular velocity, and `Entity.SetAngularVelocity` gives native/web physics a direct reset path.
- `KartController` owns a `VehicleRecovery` helper and still clears kart-specific boost and drift charge during respawn.
- `tests/main.vo::testScene3DVehicleRecovery` covers fall, upside-down, stuck, linear/angular velocity reset, steering/drift reset, on-road respawn, and track-forward alignment.
- `tests/main.vo::testScene3DKartControllerAcceptance` covers kart-specific boost/drift-charge respawn clearing on top of the generic helper.
- `voplay/rust/src/physics3d.rs` tests cover angular velocity command handling and body-state serialization.

### DBG-001: Racing Debug Overlay

voplay must expose debug drawing for racing data.

Required overlay layers:

- track centerline
- road boundaries
- gates and gate indices
- respawn poses
- surface region under vehicle
- wheel contact points
- suspension rays
- wheel slip values
- vehicle speed and grounded state

Acceptance criteria:

- Overlay can be toggled by game code without changing engine internals.
- Overlay works in native and web runner.
- Overlay drawing does not require loading debug-only assets.
- A screenshot of the reference track clearly shows centerline, boundaries, gates, and wheel contact markers.

Current coverage:

- `scene3d.SpawnTrackDebugOverlay` draws centerline, road boundaries, gates, and respawn markers with generated cube geometry or a caller-provided model.
- `scene3d.SpawnVehicleDebugOverlay` draws wheel contact points and suspension rays.
- `scene3d.SpawnRacingDebugOverlay` composes track, vehicle, and surface-under-vehicle markers and returns a `VehicleDebugSnapshot` containing speed, grounded state, surface hit, per-wheel contact, suspension, normal load, longitudinal slip, lateral slip, and surface id.
- `tests/main.vo::testScene3DVehicleTelemetryAndKartController` covers racing debug overlay spawn/update/destroy and structured debug snapshot data.
- MarbleRush wires F3 to toggle the composed racing debug overlay alongside the engine HUD.
- Visual screenshot verification against the MarbleRush reference track remains the final manual acceptance step.

## P1 Requirements

### RND-001: Material API

voplay must move beyond entity tint as the only runtime material control.

Required API:

- `MaterialID`
- material loading from GLB where available
- material override on `EntityDesc`
- albedo texture
- normal texture
- metallic-roughness texture
- normal scale
- roughness and metallic values
- emissive texture or color
- sampler configuration
- optional toon ramp or stylized shading mode

Acceptance criteria:

- A GLB with base color texture renders with the source texture visible without game-side tint tricks.
- A GLB with vertex colors preserves those colors.
- A material override can replace albedo without replacing the model.
- Missing optional maps fall back to documented defaults.
- Material resources are freed when their owning asset group is released.

Current coverage:

- `voplay.MaterialID` and `voplay.MaterialDesc` define the engine material contract: base color, albedo, normal, metallic-roughness, emissive, roughness, metallic, normal scale, UV scale, toon ramp, shading mode, texture wrap mode, and texture filter mode.
- The 3D draw stream is material-first: `DrawModel`, `DrawSkinnedModel`, and `Scene3DUpsertObject` now encode `MaterialDesc` directly instead of a tint-only payload.
- `scene3d.EntityDesc.Material` and `Entity.Material` provide per-entity material override. Untinted entities now preserve the model source material by default; color-only callers still normalize into a material base color at spawn time.
- The native/web renderer now applies source GLB base color, albedo, normal, metallic-roughness, emissive texture/factor, plus runtime albedo/normal/metallic-roughness/emissive/toon overrides, roughness, metallic, normal scale, UV scale, emissive color, and toon shading mode for static, instanced, and skinned meshes.
- Heightfield terrain uses the same material path for both single-material terrain and splat terrain. Single-material terrain supports normal/metallic-roughness textures plus scalar normal/roughness/metallic values; splat terrain supports per-layer albedo, normal, metallic-roughness, UV scale, and normal scale. TrackAsset/Map terrain data validates and packages those resources.
- The model loader now generates tangents when GLB tangent data is missing, and uploads base/emissive textures as sRGB while normal and metallic-roughness maps are uploaded as linear data.
- 3D material sampling supports explicit material sampler config: source/default, repeat, clamp, mirror, linear, and nearest. Empty materials inherit the GLB/source sampler; runtime overrides can force sampler behavior per material. Terrain splat control uses clamp/linear and terrain layers use repeat/linear.
- Missing optional maps fall back to a white bound texture plus per-material texture flags; mesh and skinned WGSL sample material maps in WebGPU-uniform control flow, so invalid optional-map branches cannot black out the source material.
- Model normal transforms now use the inverse-transpose normal matrix, so tangent-space normal maps remain correct under rotation and non-uniform scale.
- `tests/main.vo::testScene3DTintSemantics` covers material defaulting, tint-to-material normalization, and explicit material overrides including albedo, normal, metallic-roughness, emissive, roughness, metallic, normal scale, emissive color, UV scale, toon ramp, toon shading mode, wrap mode, and filter mode.
- `voplay/rust/src/pipeline3d.rs` tests cover renderer-side material parameter packing, normal-scale fallback, and sampler override resolution; `voplay/rust/src/stream.rs` tests cover the extended material draw-stream layout; `voplay/rust/src/model_loader.rs` tests cover tangent generation and missing-attribute fallback; `voplay/rust/src/math3d.rs` tests cover normal-matrix rotation and inverse-scale behavior.

### RND-002: Kart Racing Lighting Profile

voplay must provide a reusable lighting setup suitable for bright stylized outdoor racing scenes.

Required features:

- directional sun
- ambient sky color
- hemisphere or environment contribution
- shadow strength control
- tone mapping or color management suitable for textured assets
- fog that can blend distant terrain into sky

Acceptance criteria:

- The reference racing scene is neither overexposed nor underexposed under the default profile.
- Road texture color remains recognizably close to source texture under the default profile.
- Shadow enablement does not black out unlit surfaces.
- Web and native screenshots have the same overall exposure class.

Current coverage:

- `scene3d.KartRacingLightingProfile` defines the reusable outdoor racing setup: hemisphere ambient sky/ground contribution, warm directional sun, cool directional fill, linear distance fog, and shadow defaults.
- `scene3d.ColorGradingConfig` and `DrawCtx.SetColorGrading3D` expose renderer-side color management for 3D mesh output: tone map mode, exposure, contrast, and saturation.
- `scene3d.LightingProfile.ShadowStrength`, `Scene.ShadowStrength`, and `DrawCtx.SetShadow3D` expose softened stylized shadow strength so contact shadows can read without crushing unlit material color.
- `Scene.ApplyLightingProfile` applies a profile atomically and copies light slices so game-side edits cannot mutate scene state by aliasing.
- The native/web mesh, skinned mesh, and terrain shaders apply color grading after fog. The kart racing profile keeps tone mapping off by default to preserve source texture color, while applying mild exposure, contrast, and saturation lift for bright stylized outdoor scenes.
- MarbleRush now uses the voplay kart racing lighting profile and only overrides theme-specific ambient/fog colors and fog range.
- `tests/main.vo::testScene3DLightingProfile` covers profile defaults and application semantics.
- `tests/main.vo::testScene3DColorGradingSemantics` covers DrawCtx validation and scene default normalization.
- `voplay/rust/src/stream.rs` tests cover color-grading draw stream decoding.
- `tools/check_render_exposure.mjs` provides a PNG-based screenshot exposure gate for web/native comparison: both screenshots must remain balanced, avoid excessive dark/bright clipping, and stay within a documented mean-luminance delta. Its self-test covers pass/fail behavior, and the current MarbleRush web reference crop passes as balanced.

### RND-003: Decal and Road Marking Support

voplay must support road markings without baking every line into the base mesh texture.

Required uses:

- lane lines
- start line
- arrows
- boost pad markings
- skid marks

Acceptance criteria:

- A decal can be placed on a track or terrain surface without z-fighting in the reference scene.
- At least 100 simple decals can be visible without a visible correctness failure.
- Decals can be hidden or destroyed through the scene lifecycle.

Current coverage:

- `scene3d.SpawnDecal` creates a material-driven decal entity with a small surface offset to avoid z-fighting.
- `scene3d.SpawnTrackDecal` places decals from track distance and lateral offset using `Track.PoseAt`.
- Decals use normal scene entities, so they can be hidden, destroyed, and cleared through the existing scene lifecycle.
- `tests/main.vo::testScene3DDecals` covers track placement, material assignment, z-fighting offset, explicit destroy, idempotent destroy, and 100 simple decals.

### AST-001: vopack Manifest and Dependency Graph

`vopack` usage must support production track packs.

Required manifest data:

- pack name
- pack version
- asset paths
- asset type
- content hash
- source path, when available
- dependencies

Acceptance criteria:

- Loading a track from pack can verify all declared dependencies exist before spawning.
- Missing dependency errors include both the requester asset and the missing path.
- The manifest can be read without loading all asset payloads.
- Repacking unchanged source assets produces stable content hashes.

Current coverage:

- `vopack` writes `.vopack/manifest.json` metadata automatically when closing a pack.
- Manifest assets include path, inferred or explicit type, CRC32 content hash, source path for `AddFile`, and declared dependencies.
- `PackReader.Manifest` reads only the manifest metadata entry; public `Read`, `Has`, `List`, and `Len` treat the manifest as package metadata rather than a normal game resource.
- `PackReader.VerifyManifestDependencies` and `MountSet.VerifyManifestDependencies` fail before payload load/spawn when a requester has missing dependencies, and errors include requester plus missing path.
- `voplay.Assets.VerifyPackDependencies` is called by `scene3d.LoadTrackWithAssets` before reading and spawning a packed `TrackAsset`.
- MarbleRush `tools/pack_assets.vo` declares track and road-mesh dependencies in the generated asset pack.
- Covered by `vopack/tests/main.vo::testManifest`, `testManifestDependencies`, and `voplay/tests/main.vo` packed-track load/failure cases.

### AST-002: Track Baking Tool Contract

voplay must define the output contract for tools that bake source track data into runtime assets.

Required outputs:

- `track.json`
- terrain heightmap or terrain chunks
- visual track mesh
- collision track mesh
- surface metadata
- gate and respawn metadata
- generated manifest

Acceptance criteria:

- The baking tool can produce a complete reference track from source inputs.
- Running the bake twice with unchanged inputs produces byte-identical JSON and stable hashes for deterministic outputs.
- The baked output passes `ValidateTrackAsset`.
- The baked output can be packed into `vopack` and loaded in the web runner.

Current coverage:

- `scene3d.BuildTrackBakeOutput` derives the required runtime output set from a `TrackAsset`.
- `scene3d.ValidateTrackBakeOutput` enforces `track.json`, terrain, visible visual track mesh, collision track mesh, surfaces, gates, respawns, generated manifest, and runtime resources.
- `scene3d.MarshalBakedTrackAsset` emits deterministic indented JSON after `ValidateTrackAsset`.
- `scene3d.TrackBakeDependencies` returns the manifest dependency list for the baked `track.json`.
- MarbleRush `tools/pack_assets.vo` validates the demo track bake contract and uses it to populate the generated `vopack` dependency graph.
- Covered by `tests/main.vo::testScene3DTrackBakeContract`, `testScene3DTrackSpawnAndTraversability`, and MarbleRush `tools/pack_assets.vo`.

### CAM-001: Racing Camera

voplay must provide a racing camera separate from the generic third-person camera.

Required behavior:

- follow target with smoothing
- lookahead based on velocity
- speed-based distance and FOV
- drift offset
- collision avoidance
- landing shake hook
- boost FOV kick

Acceptance criteria:

- Camera stays behind the kart during normal driving.
- Camera lookahead points into turns instead of directly at the chassis.
- Camera does not clip through terrain on the reference track.
- Boost temporarily changes FOV or distance and returns smoothly.
- Camera update is deterministic for fixed input samples.

Current coverage:

- `scene3d.RacingCamera` is a dedicated racing camera controller separate from `ThirdPersonCamera`.
- `RacingCameraTarget` carries position, forward, velocity, speed, drift, boost, and landing shake input.
- Camera update supports follow smoothing, target smoothing, velocity/speed lookahead, speed-based distance, speed/boost FOV, drift side offset, landing shake vertical hook, and physics raycast collision avoidance.
- `scene3d.RacingCameraTargetFromVehicle` adapts voplay raycast vehicles into camera targets.
- Covered by `tests/main.vo::testScene3DRacingCamera` for behind-kart placement, lookahead, drift offset, boost FOV return, landing shake hook, and deterministic fixed-sample output.

### INP-001: Racing Input Profile

voplay must support keyboard and gamepad input suitable for racing.

Required features:

- analog steering
- analog throttle
- analog brake
- deadzone
- action rebinding
- gamepad detection
- optional rumble API

Acceptance criteria:

- Keyboard controls still work with digital inputs.
- Gamepad left stick maps to steering with configurable deadzone.
- Gamepad triggers map to throttle and brake.
- Rebinding an action changes both keyboard and gamepad bindings.
- Input axis values are normalized to `[-1, 1]`.

Current coverage:

- `voplay.InputState` now tracks connected gamepads, named axes, named buttons, per-frame button pressed/released state, and rumble low/high values.
- `voplay.ActionMap` supports key, pointer, gamepad button, and signed gamepad axis bindings; `ActionMap.Value` returns normalized analog action strength.
- `scene3d.RacingInputProfile` binds keyboard and gamepad defaults for steer, throttle, brake, drift, and boost.
- `scene3d.RacingInput.VehicleInput` converts normalized racing input into `VehicleInput`.
- Deadzone handling maps stick/trigger input into normalized `[0, 1]` action values and `[-1, 1]` steering axes.
- Covered by `tests/main.vo::testRacingInputProfile` for digital keyboard input, gamepad stick deadzone, trigger throttle/brake, button drift/boost, rebinding, and rumble state.

### AUD-001: Vehicle Audio Helpers

voplay must provide reusable helpers for parameterized vehicle sound.

Required behavior:

- looping engine sound controlled by RPM or speed
- tire skid sound controlled by slip
- boost sound
- collision sound hook
- surface rolling sound hook

Acceptance criteria:

- Engine loop pitch changes with speed or RPM input.
- Tire skid sound is silent below slip threshold and audible above it.
- Sounds can be attached to a moving entity and update position each frame.
- Audio helpers release sources when the entity or scene closes.

Current coverage:

- `vogui` and `voplay` audio source APIs now support `SetParams(volume, pitch)` for persistent 3D loop sources.
- Native `vogui` audio updates spatial source volume and playback speed; web bridge updates GainNode volume and BufferSource playbackRate.
- `scene3d.VehicleAudio` attaches engine, skid, boost, and surface loop sources to an entity.
- `VehicleAudio.Update` maps speed or RPM to engine pitch/volume, slip to skid volume, boost to boost loop volume, surface amount to rolling volume, and collision impulse to a one-shot collision sound.
- Attached sources follow entity position through existing `Scene.UpdateAudio` and are removed by `VehicleAudio.Destroy` or scene/entity destruction.
- Covered by `tests/main.vo::testScene3DVehicleAudioHelpers`.

## P2 Requirements

### PERF-001: Racing Scene Performance

voplay must support large repeated scene content.

Required features:

- retained 3D scene path
- instancing for repeated static meshes
- distance culling
- frustum culling
- optional LOD selection

Acceptance criteria:

- A reference scene with 1,000 repeated static props renders without per-frame full-scene serialization.
- Static props sharing model and material are eligible for batching.
- Hidden or culled objects do not issue visible draw work.
- The performance test records object count, draw count, and frame CPU timing.

Current coverage:

- `scene3d.EntityDesc` now exposes `StaticRenderable`, `CullDistance`, `CullRadius`, and `FrustumCull` as engine-level render contract fields.
- `Scene.flushRenderScene` computes effective visibility from entity visibility plus distance/frustum culling before upserting retained render objects.
- `Scene.RenderStats` reports active entities, model entities, visible entities, culled entities, upserted/destroyed retained objects, batch-eligible objects, batch group count, retained-scene flush CPU time, and scene draw CPU time.
- Static renderables sharing model/material are counted as batch eligible; culled objects are sent as hidden retained objects rather than visible draw work.
- Existing Rust `pipeline3d` already batches non-skinned static mesh draws by model/mesh/texture through instanced rendering.
- Covered by `tests/main.vo::testScene3DRenderPerformanceContract`, including 1,000 shared-model static props, recorded CPU timing, and a second retained draw with zero object upserts.

### PERF-002: Terrain and Track Chunking

Large tracks must not require one monolithic terrain or mesh.

Acceptance criteria:

- A track can reference multiple terrain or mesh chunks.
- Track queries work across chunk boundaries.
- Vehicle driving across a chunk boundary has no physics seam above documented tolerance.
- Unloaded or hidden chunks cannot be returned by collision queries unless explicitly configured.

Current coverage:

- `TrackAsset` now supports `terrainChunks[]` alongside `meshes[]`, so large tracks do not require one monolithic terrain heightfield.
- Terrain chunks use the existing `MapTerrain` runtime contract and are validated with path-specific issue locations.
- `ValidateTrackAssetResources` checks chunk heightmap and texture dependencies through filesystem or mounted `vopack` assets.
- `SpawnTrackWithAssets` spawns every terrain chunk, records them in `TrackSpawnResult.TerrainChunks`, and includes their entities in normal lifecycle cleanup.
- `TrackSpawnResult.HeightProbe` composes the spawned primary terrain and chunk terrains into a traversability height probe, skipping inactive or hidden chunks.
- `scene3d.BuildTrackBakeOutput` includes terrain chunk heightmaps/textures in baked output resources and manifest dependencies.
- Track pose/surface/gate/respawn queries remain based on track distance and centerline, so they are continuous across chunk boundaries.
- Covered by `tests/main.vo::testScene3DTrackTerrainChunks`, including chunk spawn/cleanup, traversability height probing across chunks, and hidden chunk exclusion.

### AI-001: Racing Line Support

voplay should expose reusable racing-line data, but not AI behavior rules.

Acceptance criteria:

- Track asset can contain one or more racing lines.
- `Track.PoseOnLine(lineName, distance)` returns pose and target speed hint.
- Missing line names return deterministic errors.
- MarbleRush can use the racing line data without hardcoding path points in game code.

Current coverage:

- `TrackAsset` now includes `racingLines[]` with named distance/lateral/targetSpeed points.
- `ValidateTrackAsset` checks racing line names, minimum point count, finite values, in-range distances, strictly increasing distances, and non-negative target speeds.
- `Track.PoseOnLine(lineName, distance)` returns an interpolated `TrackRacingLinePose` with centerline pose offset by lateral distance and an interpolated target speed hint.
- Missing line names return deterministic errors including the missing name.
- Covered by `tests/main.vo::testScene3DTrackValidationAndQueries`.

### REP-001: Replay and Ghost Data Contract

voplay should define deterministic input and transform recording helpers.

Acceptance criteria:

- Fixed-step input samples can be recorded and replayed.
- Replay format records engine version, fixed step, input samples, and optional checkpoints.
- A replay on the same build reaches the same checkpoint sequence on the reference track.

Current coverage:

- `scene3d.RacingReplay` records format version, engine version, fixed step, input samples, transform samples, and optional checkpoints.
- `RacingReplayRecorder` records normalized fixed-step `RacingInput`, transforms, and checkpoint events.
- `MarshalRacingReplay` and `UnmarshalRacingReplay` provide deterministic JSON serialization with validation.
- `RacingReplayCursor` replays input samples and checkpoint events by fixed step.
- `CheckpointSequence` exposes deterministic checkpoint order for same-build comparisons.
- Covered by `tests/main.vo::testRacingReplayContract`.

## Recommended Implementation Order

1. TRK-001, TRK-003, and AST-002: define schema, validation, and bake contract first.
2. TRK-002 and TRK-004: create runtime `Track` and spawn it from files and `vopack`.
3. TRK-005 and DBG-001: add traversability validation and debug overlay before building more maps.
4. VEH-001 and VEH-002: upgrade vehicle telemetry and kart controller against the reference track.
5. RND-001 and RND-002: make materials and lighting good enough that art assets survive import.
6. CAM-001 and INP-001: make the game feel playable with camera and controller support.
7. AST-001, PERF-001, and PERF-002: scale the asset and runtime path for larger tracks.
8. AUD-001, AI-001, and REP-001: add production polish and advanced systems.

## Reference Demos Required

The plan maintains these demos through `tools/racing_reference_demos.vo`.

- `track_minimal`: smallest valid track asset.
- `track_validation_broken`: intentionally invalid track for validator tests.
- `track_reference_oval`: closed loop with road, grass, gates, respawns, and boost surface.
- `kart_vehicle_reference`: single kart using `KartController` on the reference oval.
- `racing_debug_overlay`: same scene with debug overlay enabled.

Acceptance command:

```sh
vo run tools/racing_reference_demos.vo
```

The command must print `VO:REFERENCE_DEMOS PASS all`. A failure must identify the demo name and the broken contract.

## Stop Conditions

Do not continue building MarbleRush content on top of the old map-only path once TRK-001 starts. New racing content should use `TrackAsset`, even if early versions still wrap existing terrain and mesh files.

Do not add MarbleRush-specific fields to voplay schemas unless the field is generic to racing games. Game-specific data should live under an extension field that voplay preserves but does not interpret.
