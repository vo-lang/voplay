module = "github.com/vo-lang/voplay"
vo = "^0.1.0"

[dependencies]
"github.com/vo-lang/vogui" = "^0.1.0"
"github.com/vo-lang/vopack" = "^0.1.0"

[extension]
name = "voplay"

[extension.native]
library = "vo_voplay"
targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]

[extension.wasm]
kind = "bindgen"
wasm = "voplay_island_bg.wasm"
js = "voplay_island.js"

[extension.web]
entry = "Run"
capabilities = ["island_transport", "vfs", "vo_web", "widget"]

[extension.web.js]
renderer = "js/dist/voplay-render-island.js"

[build.native]
kind = "cargo"
manifest = "rust/Cargo.toml"
package = "vo-voplay"

[build.wasm]
wasm = "web-artifacts/voplay_island_bg.wasm"
js = "web-artifacts/voplay_island.js"
