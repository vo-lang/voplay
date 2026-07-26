// Local browser artifact loader for the Voplay workspace extension.
export const voProviderEntrypoints = Object.freeze([
  "voplay-audio-worker",
  "voplay-render-worker",
]);

export async function instantiateVoProvider(source, imports = {}) {
  const bytes = source instanceof ArrayBuffer
    ? source
    : await (await fetch(
      source ?? new URL("voplay_extension_bg.wasm", import.meta.url),
    )).arrayBuffer();
  const result = await WebAssembly.instantiate(bytes, imports);
  return result.instance ?? result;
}
