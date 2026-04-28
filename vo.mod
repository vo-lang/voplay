module github.com/vo-lang/voplay

vo ^0.1.0

require github.com/vo-lang/vogui v0.1.13
require github.com/vo-lang/vopack v0.1.1

[extension]
name = "voplay"
include = [
  "js/dist",
]

[extension.native]
path = "rust/target/{profile}/libvo_voplay"

[[extension.native.targets]]
target = "aarch64-apple-darwin"
library = "libvo_voplay.dylib"

[[extension.native.targets]]
target = "x86_64-unknown-linux-gnu"
library = "libvo_voplay.so"

[extension.wasm]
type = "bindgen"
wasm = "voplay_island_bg.wasm"
js_glue = "voplay_island.js"
local_wasm = "web-artifacts/voplay_island_bg.wasm"
local_js_glue = "web-artifacts/voplay_island.js"

[extension.web]
entry = "Run"
capabilities = ["widget", "island_transport", "vo_web", "vfs"]

[extension.web.js]
renderer = "js/dist/voplay-render-island.js"
