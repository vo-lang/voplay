# TrackAsset v1

`TrackAsset` is the voplay racing-track data contract used by kart games.

## Coordinate System

- Units are meters.
- World up is `+Y`.
- Forward along vehicles and track poses is `-Z`.
- Right is `+X` in local vehicle space.
- Yaw is measured in radians around `+Y`.
- Positive lateral distance means right of the centerline when looking along track forward.
- `centerline[].position` stores sampled world-space control points.
- A closed loop must repeat its first centerline point as the final point within tolerance `0.05`.

## Required Fields

- `version`: must be `1`.
- `name`: non-empty track name.
- `closedLoop`: whether distance wraps.
- `terrain` or at least one `meshes[]` entry.
- `centerline[]`: at least two points; closed loops need at least three.
- `centerline[].width`: road width in meters at that point.

## Runtime Resources

Resource paths are relative to the track file directory unless absolute:

- `terrain.heightmap`
- `terrain.texture`
- `terrain.normal`
- `terrain.metallicRoughness`
- `terrain.splat.control`
- `terrain.splat.layers[].texture`
- `terrain.splat.layers[].normal`
- `terrain.splat.layers[].metallicRoughness`
- `meshes[].model`
- `meshes[].collisionModel`

`LoadTrackAsset` validates filesystem resources. `LoadTrackWithAssets` validates resources through `voplay.Assets`, so mounted `vopack` paths are checked before spawning.

Tools can call `ValidateTrackAssetFiles(asset, baseDir)` for filesystem checks or `ValidateTrackAssetResources(asset, assets, baseDir)` for mounted pack checks without spawning the track.

## Track Semantics

- `surfaces[]` apply by distance and lateral range. Higher `priority` wins.
- `kind: "road"` marks a driveable road surface.
- `kind: "offroad"` marks lower-grip terrain.
- `gates[]` must be strictly increasing by `distance`.
- `spawns[]` define named start poses.
- `respawns[]` define recovery poses aligned to the track.
- `triggers[]` define game-facing racing regions such as lap lines.
- `metadata[]` stores authoring notes as `{ "key": "...", "value": "..." }` entries.
