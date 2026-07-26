format = 1
module = "github.com/vo-lang/voplay"
version = "0.1.0"
vo = "0.1.0"
default_profile = "full"

[capabilities.core]
packages = ["github.com/vo-lang/voplay/vo/assets", "github.com/vo-lang/voplay/vo/core", "github.com/vo-lang/voplay/vo/input", "github.com/vo-lang/voplay/vo/plugin", "github.com/vo-lang/voplay/vo/schedule", "github.com/vo-lang/voplay/vo/world"]

[capabilities.render2d]
packages = ["github.com/vo-lang/voplay/vo/render2d"]

[capabilities.text]
packages = ["github.com/vo-lang/voplay/vo/render"]

[capabilities.image]
packages = ["github.com/vo-lang/voplay/vo/render"]

[capabilities.render3d]
requires = ["render2d"]
packages = ["github.com/vo-lang/voplay/vo/render3d", "github.com/vo-lang/voplay/vo/scene"]

[capabilities.gltf]
requires = ["render3d"]
packages = ["github.com/vo-lang/voplay/vo/render3d"]

[capabilities.physics2d]
packages = ["github.com/vo-lang/voplay/vo/physics2d"]

[capabilities.physics3d]
packages = ["github.com/vo-lang/voplay/vo/physics3d"]

[capabilities.animation]
packages = ["github.com/vo-lang/voplay/vo/animation"]

[capabilities.audio]
packages = ["github.com/vo-lang/voplay/vo/audio"]

[capabilities.pack]

[capabilities.readback]
packages = ["github.com/vo-lang/voplay/vo/render"]

[capabilities.inspection]
packages = ["github.com/vo-lang/voplay/vo/diagnostics"]

[capabilities.frame-debug-capture]
requires = ["inspection"]
packages = ["github.com/vo-lang/voplay/vo/editor"]

[capabilities.shader-diagnostics]
requires = ["inspection"]
packages = ["github.com/vo-lang/voplay/vo/editor"]

[profiles.core]
capabilities = ["core"]

[profiles.2d]
capabilities = ["core", "image", "physics2d", "readback", "render2d", "text"]

[profiles.3d]
capabilities = ["animation", "core", "gltf", "image", "physics3d", "readback", "render2d", "render3d", "text"]

[profiles.full]
capabilities = ["animation", "audio", "core", "gltf", "image", "pack", "physics2d", "physics3d", "readback", "render2d", "render3d", "text"]

[profiles.editor]
extends = "full"
capabilities = ["frame-debug-capture", "inspection", "shader-diagnostics"]

[extension]
name = "voplay"

[extension.wasm]
kind = "standalone"
wasm = "voplay_extension_bg.wasm"

[build.wasm]
wasm = "rust/pkg-web/voplay_extension_bg.wasm"

[extension.web]
provider_role = "game-logic"
provider_roles = ["game-logic", "game-asset", "game-renderer", "game-audio"]
capabilities = ["app_surface", "asset_buffer", "framework_lane", "voplay.engine-pause", "voplay.engine-resume", "voplay.engine-shutdown", "voplay.engine-start", "voplay.engine-step", "voplay.install-entry", "voplay.new-engine", "voplay.run-entry", "voplay.target-commit-ticks", "voplay.target-next-ticks", "voplay.target-start"]

[extension.web.js]
asset = "web/src/asset_provider.ts"
audio = "web/src/audio_provider.ts"
protocol = "protocol/generated/voplay_protocol.ts"
renderer = "web/src/studio_renderer.ts"

[[extension.source_recipes]]
derive = true
capabilities = ["core"]
target = "aarch64-apple-darwin"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3"]
abi_inputs = ["voplay-provider-abi-v2"]
vo_packages = ["voplay/assets", "voplay/core", "voplay/input", "voplay/plugin", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-assets", "crate:voplay-protocol", "crate:voplay-runtime"]
role_outputs = [
  { role = "logic", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "asset", kind = "extension-native", name = "libvoplay_extension.dylib" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["core", "image", "physics2d", "readback", "render2d", "text"]
target = "aarch64-apple-darwin"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/assets", "voplay/core", "voplay/input", "voplay/physics2d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-assets", "crate:voplay-physics-2d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-core", "crate:voplay-runtime"]
role_outputs = [
  { role = "logic", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "asset", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "render", kind = "extension-native", name = "libvoplay_extension.dylib" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "core", "gltf", "image", "physics3d", "readback", "render2d", "render3d", "text"]
target = "aarch64-apple-darwin"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/core", "voplay/input", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-import-gltf", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
role_outputs = [
  { role = "logic", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "asset", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "render", kind = "extension-native", name = "libvoplay_extension.dylib" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "audio", "core", "gltf", "image", "pack", "physics2d", "physics3d", "readback", "render2d", "render3d", "text"]
target = "aarch64-apple-darwin"
toolchain = "0.1.4"
schema_inputs = ["voplay-audio-protocol-v1", "voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-audio-abi-v1", "voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/audio", "voplay/core", "voplay/input", "voplay/physics2d", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-audio", "crate:voplay-import-gltf", "crate:voplay-physics-2d", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
role_outputs = [
  { role = "logic", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "asset", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "render", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "audio", kind = "extension-native", name = "libvoplay_extension.dylib" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "audio", "core", "frame-debug-capture", "gltf", "image", "inspection", "pack", "physics2d", "physics3d", "readback", "render2d", "render3d", "shader-diagnostics", "text"]
target = "aarch64-apple-darwin"
toolchain = "0.1.4"
schema_inputs = ["voplay-audio-protocol-v1", "voplay-inspection-protocol-v1", "voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-audio-abi-v1", "voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/audio", "voplay/core", "voplay/diagnostics", "voplay/editor", "voplay/input", "voplay/physics2d", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-audio", "crate:voplay-import-gltf", "crate:voplay-physics-2d", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "crate:voplay-vogui-editor", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
role_outputs = [
  { role = "logic", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "asset", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "render", kind = "extension-native", name = "libvoplay_extension.dylib" },
  { role = "audio", kind = "extension-native", name = "libvoplay_extension.dylib" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["core"]
target = "wasm32-unknown-unknown"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3"]
abi_inputs = ["voplay-provider-abi-v2"]
vo_packages = ["voplay/assets", "voplay/core", "voplay/input", "voplay/plugin", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-assets", "crate:voplay-protocol", "crate:voplay-runtime"]
role_outputs = [
  { role = "logic", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "logic", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "asset", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "asset", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["core", "image", "physics2d", "readback", "render2d", "text"]
target = "wasm32-unknown-unknown"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/assets", "voplay/core", "voplay/input", "voplay/physics2d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-assets", "crate:voplay-physics-2d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-core", "crate:voplay-runtime"]
js_entrypoints = ["voplay-render-worker"]
role_outputs = [
  { role = "logic", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "logic", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "asset", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "asset", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "render", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "render", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "core", "gltf", "image", "physics3d", "readback", "render2d", "render3d", "text"]
target = "wasm32-unknown-unknown"
toolchain = "0.1.4"
schema_inputs = ["voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/core", "voplay/input", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-import-gltf", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
js_entrypoints = ["voplay-render-worker"]
role_outputs = [
  { role = "logic", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "logic", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "asset", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "asset", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "render", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "render", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "audio", "core", "gltf", "image", "pack", "physics2d", "physics3d", "readback", "render2d", "render3d", "text"]
target = "wasm32-unknown-unknown"
toolchain = "0.1.4"
schema_inputs = ["voplay-audio-protocol-v1", "voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-audio-abi-v1", "voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/audio", "voplay/core", "voplay/input", "voplay/physics2d", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-audio", "crate:voplay-import-gltf", "crate:voplay-physics-2d", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
js_entrypoints = ["voplay-audio-worker", "voplay-render-worker"]
role_outputs = [
  { role = "logic", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "logic", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "asset", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "asset", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "render", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "render", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "audio", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "audio", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
]

[[extension.source_recipes]]
derive = true
capabilities = ["animation", "audio", "core", "frame-debug-capture", "gltf", "image", "inspection", "pack", "physics2d", "physics3d", "readback", "render2d", "render3d", "shader-diagnostics", "text"]
target = "wasm32-unknown-unknown"
toolchain = "0.1.4"
schema_inputs = ["voplay-audio-protocol-v1", "voplay-inspection-protocol-v1", "voplay-protocol-v3", "voplay-render-protocol-v1"]
abi_inputs = ["voplay-audio-abi-v1", "voplay-provider-abi-v2", "voplay-render-abi-v1"]
vo_packages = ["voplay/animation", "voplay/assets", "voplay/audio", "voplay/core", "voplay/diagnostics", "voplay/editor", "voplay/input", "voplay/physics2d", "voplay/physics3d", "voplay/plugin", "voplay/render", "voplay/render2d", "voplay/render3d", "voplay/scene", "voplay/schedule", "voplay/world"]
cargo_features = ["crate:voplay-animation", "crate:voplay-assets", "crate:voplay-audio", "crate:voplay-import-gltf", "crate:voplay-physics-2d", "crate:voplay-physics-3d", "crate:voplay-protocol", "crate:voplay-render-2d", "crate:voplay-render-3d", "crate:voplay-render-core", "crate:voplay-runtime", "crate:voplay-vogui-editor", "render-feature-factory:voplay_render_3d::builtin_overlay_feature_factory"]
js_entrypoints = ["voplay-audio-worker", "voplay-editor-inspection", "voplay-render-worker"]
role_outputs = [
  { role = "logic", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "logic", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "asset", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "asset", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "render", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "render", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
  { role = "audio", kind = "extension-js-glue", name = "voplay_extension.js" },
  { role = "audio", kind = "extension-wasm", name = "voplay_extension_bg.wasm" },
]

[[extension.generator]]
name = "voplay.component-store"
version = "13"
schema_kind = "voplay.components"

[extension.generator.artifacts]
"aarch64-apple-darwin" = "voplay-generator-provider"
"x86_64-apple-darwin" = "voplay-generator-provider"
"aarch64-unknown-linux-gnu" = "voplay-generator-provider"
"x86_64-unknown-linux-gnu" = "voplay-generator-provider"
"x86_64-pc-windows-msvc" = "voplay-generator-provider.exe"
