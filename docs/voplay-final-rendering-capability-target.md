# voplay final rendering capability target

## Outcome

voplay must be able to ship a modern stylized kart-racing scene like
`BlockKart/docs/images/terrain-upgrade-concept-v1.png` without one-off shader
hacks in the game project.

The target frame is not defined by a color palette. It is defined by a coherent
rendering stack:

- high-resolution terrain material detail that stays sharp near the camera
- broad macro color variation that removes obvious tiling at gameplay distance
- slope-, height-, curvature-, and mask-aware material blending
- road-edge dirt, gravel, curb, and shoulder transitions that sit on terrain
- contact shadows and ambient occlusion that anchor props, road pieces, wheels,
  fences, rocks, grass, flowers, and trees
- stable lighting, fog, color grading, and shadow controls that work the same
  for native and web targets
- visual terrain and physics terrain generated from the same source data

## Renderer Contract

The Vo API remains high-level. Scene authors describe terrain, roads, materials,
decals, scatter, lighting, and physics in `scene3d`. The Rust renderer owns GPU
binding layout, render passes, batching, streaming, and quality-specific shader
branches.

The engine must support both retained scene rendering and direct frame commands.
Large static racing scenes should render through retained objects, material
sorting, instancing, culling, and GPU-side batching.

## Terrain

Terrain must support:

- 16-bit or better heightmaps, with visual mesh and physics heightfield derived
  from the same decoded height data
- splat maps with at least four material layers
- per-layer albedo, normal, metallic-roughness, UV scale, normal scale, and
  sampler controls
- material tuning parameters passed from Vo to Rust to WGSL, not hardcoded in
  the shader
- slope-aware layer redistribution for exposed dirt, rock, cliffs, and worn
  shoulders
- height and curvature masks for wet lowlands, hill highlights, path wear, and
  ledge exposure
- macro variation, micro detail, and anti-tiling controls
- projected or triplanar sampling for steep slopes and cliffs
- terrain debug views for control weights, slope masks, macro color, normals,
  roughness, and physics height

## Roads And Terrain Contact

Roads must be first-class scene surfaces, not visually detached meshes. The
engine must support:

- road-edge decals and material masks
- gravel, dirt, grass, and curb blend bands along generated track edges
- road grime, tire marks, chipped curb paint, and scattered stones as projected
  decals
- physics surface metadata that matches visual surface metadata
- debug rendering for drivable surface, off-road, respawn, and collision
  boundaries

## Decals

The renderer must support projected decals on terrain and meshes:

- layer sorting and draw ordering
- albedo, normal, roughness, opacity, and mask textures
- terrain-only, mesh-only, or mixed receivers
- distance and angle fade
- atlas-friendly batching
- debug views for bounds, receiver mask, and overdraw

## Scatter And Vegetation

Large scenes must support dense detail without hand-placing every object:

- instanced scatter driven by density masks, slope masks, height ranges, and
  exclusion masks
- deterministic randomization for scale, rotation, color, bend, and material
  variant
- LOD meshes, impostors, and billboards where appropriate
- wind animation for grass and foliage
- shadow and contact-shadow participation by quality mode

## Materials

voplay materials must cover stylized PBR:

- albedo, normal, metallic-roughness, emissive, toon ramp, and optional mask
  textures
- sampler configuration per material
- per-material quality controls for texture detail, normal strength, macro
  blending, roughness response, and toon ramp response
- material preview scenes and debug channels
- atlas support for repeated small props and decals

## Lighting And Shadows

Lighting must support concept-quality outdoor racing scenes:

- directional sun with stable shadow controls
- cascaded shadow maps for racing-camera scale
- PCF or equivalent soft filtering
- contact shadows or screen-space/contact AO for objects close to terrain
- hemisphere ambient and ground bounce
- fog and atmospheric controls that preserve readable gameplay silhouettes
- debug views for shadow cascades, shadow factor, direct light, ambient, normal,
  roughness, metallic, and albedo

## Post And Presentation

The default output path must include:

- stable tone mapping
- color grading controls
- fog/atmosphere
- controlled bloom for highlights, not blanket glow
- anti-aliasing suitable for thin fences, curb edges, road markings, grass
  cards, and distant silhouettes

## Tooling

The engine needs visual development tools, not only code knobs:

- capture boards for named camera viewpoints and material states
- render debug toggles available from Vo and the host
- automated screenshot checks for blank frames, broken assets, shader errors,
  and gross visual regressions
- asset validation for missing textures, mismatched color spaces, invalid
  heightmaps, and impossible material parameters
- performance overlays for draw calls, batches, texture memory, shadow cost, and
  terrain/scatter counts
- engine-level performance diagnostics, HUD, spike attribution, and automated
  reports are specified in
  [`perf-diagnostics-final-design.md`](perf-diagnostics-final-design.md)

## Acceptance Bar

The same BlockKart gameplay camera must be able to show road, curbs, road-edge
dirt, grass, flowers, rocks, fences, trees, hills, shadows, and terrain detail
with one coherent lighting/material model. The game project may tune authored
assets, but it must not need custom renderer hacks to reach the reference
quality.

## Implementation Checklist

This checklist is the working gate. A checked item means the capability exists
as a reusable engine feature, not only as a BlockKart-specific visual tweak.

### Completed Foundation

- [x] Four-layer terrain splat materials with per-layer albedo, normal,
  metallic-roughness, UV scale, and normal scale.
- [x] Terrain material tuning transported from Vo to Rust to WGSL.
- [x] Slope-, height-, and curvature-aware terrain material redistribution.
- [x] Terrain macro variation, micro detail, anti-tiling, and steep-slope
  projected sampling.
- [x] Terrain material debug channels for masks and surface properties.
- [x] Mipmapped linear texture upload and anisotropic terrain/material sampling.
- [x] Explicit non-sRGB texture loading for normal, roughness, metallic, mask,
  height-style data through root API, Assets, split-island proxy, and wasm
  bindgen.
- [x] Raw RGBA8 texture upload for both sRGB and linear data through root API,
  texture backend abstraction, split-island proxy, renderer externs, and wasm
  bindgen so runtime/offline generators can create albedo, normal, roughness,
  metallic, mask, and height-style atlases without PNG/JPEG encoding.
- [x] MSAA main 3D rendering path.
- [x] Basic post pass with controlled bloom, sharpening, and FXAA.
- [x] Directional shadow map with PCF-style filtering, strength, softness,
  distance, and fade controls.
- [x] Directional shadow quality tier in scene3d, draw stream, renderer uniform,
  and WGSL PCF path, including off/low/medium/high/cinematic sample budgets.
- [x] Stabilized directional shadow projection snapped to the active shadow-map
  texel grid to reduce racing-camera shimmer.
- [x] Atlas-based cascaded shadow maps for high/cinematic shadow quality modes,
  including cascade split selection in shader and multi-viewport shadow
  rendering.
- [x] Sampleable camera depth prepass for screen-space lighting and depth-aware
  post effects.
- [x] First-pass screen-space contact AO driven by camera depth.
- [x] Depth-derived normal/plane-aware contact AO with camera-depth-scaled
  radius to reduce same-surface haloing on slopes.
- [x] Dual-scale contact AO production controls: broad radius/depth response,
  detail radius/strength, and surface-normal bias as first-class scene settings.
- [x] Scene-level contact AO quality presets for off/low/medium/high/cinematic
  profiles through the single quality-tier API.
- [x] Contact AO quality tier is part of the draw command and shader uniform,
  changing broad/detail sample counts per tier rather than only changing values.
- [x] Generated track surface detail layers for reusable road-edge bands and
  projected decal batches with shared scene lifecycle.
- [x] First-pass renderer-level projected decals using camera depth,
  world-position reconstruction, projection volume clipping, edge fade,
  albedo/opacity sampling, distance fade, and fixed-slot multi-atlas batching.
- [x] Renderer-level projected decal receiver masks backed by a main-pass
  receiver-mask render target for terrain-only, mesh-only, and mixed decals.
- [x] Main-pass surface properties buffer carrying encoded world normal and
  roughness into post effects.
- [x] First-pass projected decal normal/roughness response driven by surface
  properties and per-decal response controls.
- [x] Renderer-level projected decals with authored normal and roughness atlas
  textures, shared slot batching, default material maps, and retained scene3d
  fields.
- [x] Projected decal independent mask atlas sampling and receiver angle fade
  controls for cleaner road-edge dirt, tire marks, gravel, and curb wear.
- [x] Projected decal normal/roughness response uses the scene's primary
  directional light instead of a fixed post-process light vector.
- [x] Projected decal normal/roughness response can use up to three scene
  lights in the post pass, including directional and point lights with per-light
  color and point-light falloff, so sun/fill/rim/practical lights affect decal
  relief consistently instead of relying on a single presentation vector.
- [x] Projected decal composition is integrated into the presentation sample
  path: decal-resolved scene samples feed FXAA, sharpening, bloom, contact AO,
  receiver masks, surface normals, roughness response, and scene-light material
  response instead of being only a center-pixel color overlay after lighting.
- [x] Primitive/scatter instances carry render controls through Vo stream,
  retained scene state, resident chunks, GPU instance buffers, and WGSL:
  no-shadow, LOD near/far, and per-instance wind strength.
- [x] Track scatter prototypes preserve render controls so generated grass,
  flowers, stones, and trackside props can use deterministic placement with
  LOD, wind, and shadow participation rules.
- [x] Generic scatter fields for rectangular terrain regions with deterministic
  grid/count/density sampling, density masks, exclusion masks, height filters,
  slope filters, prototype weighting, and primitive layer chunking.
- [x] Primitive vegetation cards support atlas UV rectangles, Y/full billboard
  facing, alpha cutout, no back-face culling, LOD, wind, and scatter prototype
  propagation through Vo stream, renderer state, GPU instance buffers, and WGSL.
- [x] Material/decal atlas support for repeated small props and detail cards:
  projected decals use authored atlas maps, while primitive card instances can
  select atlas cells per object without changing mesh or material IDs.
- [x] Scene-level rendering quality profiles for low/medium/high/cinematic
  budgets, deterministic platform selection, shadow/contact-AO application,
  presentation-pass budgets, diagnostics, and material/terrain detail scalars.
- [x] Quality-driven vegetation scatter scaling: track and field scatter can
  scale density and LOD distance by scene quality, apply explicit density
  multipliers, and gate expensive prototypes or bands by min/max quality.
- [x] Impostor atlas authoring foundation for distant vegetation and complex
  props: deterministic atlas cell layout, padded UV and pixel rect generation,
  per-view bake camera plans, atlas material defaults, and scatter prototype
  conversion with billboard, no-shadow, LOD, quality, and atlas-UV controls.
- [x] Impostor bake backend contract and convenience APIs: scene3d can submit a
  generated bake plan to an installed renderer/offline backend, normalize the
  returned atlas texture, and emit baked scatter prototypes without game-level
  renderer hacks.
- [x] Default impostor atlas bake path: generated bake plans produce real
  albedo, normal, metallic-roughness, and mask atlas texture assets through the
  Rust/offline baker, then return baked scatter prototypes from that single v1
  path.
- [x] Mesh-aware impostor atlas baker: scene3d exports real indexed triangle
  data, projects it per atlas view, resolves visibility with a CPU z-buffer,
  antialiases coverage, and emits mesh-derived albedo, normal, roughness, and
  mask maps instead of procedural bounds silhouettes.
- [x] Single v1 model geometry export for physics and baker systems:
  `ModelGeometryBytes` carries positions,
  normals, UVs, vertex colors, material records, and per-triangle material
  indices through root API, split-island proxy, Rust externs, and wasm bindgen.
- [x] Impostor geometry baker consumes exported material state,
  including base color, metallic, roughness, normal scale, UV scale, emissive,
  detail response, roughness response, and toon response, so generated atlas
  maps follow the model's material authoring instead of a flat tint.
- [x] Single v1 texture pixel export: `TexturePixelsBytes` exposes width,
  height, color-space flags, and base-level RGBA8 data through the root API,
  texture backend abstraction, split-island proxy, Rust externs, and wasm
  bindgen.
- [x] Impostor geometry baker samples exported material textures by UV,
  including albedo, normal, metallic-roughness, emissive, and mask maps, so CPU
  generated atlases can carry authored surface detail instead of only material
  constants.
- [x] Rust/offline impostor baker: scene3d packages a single v1 bake request
  with atlas views, model geometry, and source texture pixel payloads; Rust
  rasterizes real indexed model geometry with z-buffer visibility, antialiasing,
  material texture sampling, mesh normals, roughness/metallic/mask output, and
  returns albedo, normal, metallic-roughness, and mask atlas pixels for upload.
- [x] Track surface authoring plan generated from `TrackSurface` data:
  off-road edge-transition meshes, road/boost/off-road decal batches, spawned
  generated visual details, and copied physics surface metadata come from the
  same authored distance/lateral ranges.
- [x] Stable presentation stack for atmosphere, fog, color grading, and
  post-process: reusable presets, scene application/capture, fog normalization,
  and kart lighting profile reuse the same presentation source.

### Completed Concept-Grade Systems

- [x] Full presentation-integrated projected decal composition: decal-resolved
  scene color participates in FXAA, sharpening, bloom taps, contact AO, and
  presentation sampling instead of being a late center-pixel overlay.
- [x] Stylized PBR material response with toon ramp, emissive, mask channels,
  material detail, macro blend, roughness response, toon response, primitive
  preset propagation, glTF occlusion-as-mask loading, and static/skinned shader
  support.
- [x] Physics and visual terrain generated from the same decoded high-precision
  height grid with source metadata, checksums, range stats, and debug comparison
  for visual/physics heightfield mismatches.

### Completed Concept-Critical Tooling

- [x] Visual development tooling: capture/debug boards with camera,
  presentation, quality, diagnostics/performance state; material and terrain
  validation; screenshot blank/flat-frame regression checks; and scene-level
  visual asset issue reporting.
