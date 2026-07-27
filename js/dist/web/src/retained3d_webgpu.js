const BUFFER_COPY_DST = 0x08;
const BUFFER_INDEX = 0x10;
const BUFFER_VERTEX = 0x20;
const BUFFER_UNIFORM = 0x40;
const TEXTURE_COPY_DST = 0x02;
const TEXTURE_BINDING = 0x04;
const TEXTURE_RENDER_ATTACHMENT = 0x10;
const MAX_INSTANCES = 65_536;
const MAX_OVERLAY_VERTICES = 262_144;
const PRESENT_SAMPLE_COUNT = 4;
export class WebGpuRetainedRenderer {
    #canvas;
    #device;
    #context;
    #format;
    #scenePipeline;
    #shadowPipeline;
    #skyPipeline;
    #overlayPipeline;
    #uniform;
    #uniformBindGroup;
    #shadowUniformBindGroup;
    #shadowTexture;
    #shadowSampler;
    #sampler;
    #whiteTexture;
    #whiteTextureView;
    #fallbackMaterialParameters;
    #fallbackMaterialBindGroup;
    #meshes = new Map();
    #materials = new Map();
    #textures = new Map();
    #instanceBatches = new Map();
    #preparedBatches = [];
    #overlayVertexCount = 0;
    #overlayScene = [];
    #overlayWidth = 0;
    #overlayHeight = 0;
    #overlayBuffer;
    #overlayCapacity = 6;
    #depth = null;
    #colorMsaa = null;
    #width = 0;
    #height = 0;
    #validationFrames = 8;
    #previousScene = null;
    #scene = null;
    #sceneReceivedAt = 0;
    #sceneIntervalMillis = 100;
    #animationFrame = 0;
    #drawing = false;
    #presentError = null;
    #presentFrames = 0;
    #presentCpuMillis = 0;
    #presentStatsStarted = 0;
    #lastPresentAt = 0;
    #closed = false;
    static async create(canvas) {
        const gpu = navigator.gpu;
        if (gpu === undefined)
            throw new Error("Voplay retained 3D requires WebGPU");
        const adapter = await gpu.requestAdapter({ powerPreference: "high-performance" });
        if (adapter === null)
            throw new Error("Voplay retained 3D has no WebGPU adapter");
        const device = await adapter.requestDevice();
        const context = canvas.getContext("webgpu");
        if (context === null) {
            device.destroy();
            throw new Error("Voplay retained 3D cannot acquire a WebGPU canvas");
        }
        device.pushErrorScope("validation");
        const renderer = new WebGpuRetainedRenderer(canvas, device, context, gpu.getPreferredCanvasFormat());
        const validation = await device.popErrorScope();
        if (validation !== null) {
            renderer.close();
            throw new Error(`Voplay retained 3D resource creation failed: ${validation.message ?? "unknown"}`);
        }
        return renderer;
    }
    constructor(canvas, device, context, format) {
        this.#canvas = canvas;
        this.#device = device;
        this.#context = context;
        this.#format = format;
        this.#scenePipeline = createScenePipeline(device, format);
        this.#shadowPipeline = createShadowPipeline(device);
        this.#skyPipeline = createSkyPipeline(device, format);
        this.#overlayPipeline = createOverlayPipeline(device, format);
        this.#uniform = device.createBuffer({
            label: "Voplay retained 3D scene uniform",
            size: 192,
            usage: BUFFER_UNIFORM | BUFFER_COPY_DST,
        });
        this.#shadowTexture = device.createTexture({
            label: "Voplay retained 3D sun shadow",
            size: [2048, 2048, 1],
            format: "depth24plus",
            usage: TEXTURE_BINDING | TEXTURE_RENDER_ATTACHMENT,
        });
        this.#shadowSampler = device.createSampler({
            compare: "less",
            magFilter: "nearest",
            minFilter: "nearest",
        });
        this.#uniformBindGroup = device.createBindGroup({
            label: "Voplay retained 3D scene bind group",
            layout: this.#scenePipeline.getBindGroupLayout(0),
            entries: [
                { binding: 0, resource: { buffer: this.#uniform } },
                { binding: 1, resource: this.#shadowTexture.createView() },
                { binding: 2, resource: this.#shadowSampler },
            ],
        });
        this.#shadowUniformBindGroup = device.createBindGroup({
            label: "Voplay retained 3D shadow bind group",
            layout: this.#shadowPipeline.getBindGroupLayout(0),
            entries: [{ binding: 0, resource: { buffer: this.#uniform } }],
        });
        this.#sampler = device.createSampler({
            label: "Voplay retained 3D material sampler",
            addressModeU: "repeat",
            addressModeV: "repeat",
            magFilter: "linear",
            minFilter: "linear",
            mipmapFilter: "linear",
        });
        this.#whiteTexture = device.createTexture({
            label: "Voplay retained 3D white texture",
            size: [1, 1, 1],
            format: "rgba8unorm",
            usage: TEXTURE_BINDING | TEXTURE_COPY_DST,
        });
        this.#whiteTextureView = this.#whiteTexture.createView();
        device.queue.writeTexture({ texture: this.#whiteTexture }, new Uint8Array([255, 255, 255, 255]), { bytesPerRow: 4 }, [1, 1, 1]);
        this.#fallbackMaterialParameters = device.createBuffer({
            label: "Voplay retained 3D fallback material parameters",
            size: 48,
            usage: BUFFER_UNIFORM | BUFFER_COPY_DST,
        });
        device.queue.writeBuffer(this.#fallbackMaterialParameters, 0, new Float32Array([0, 1, 0, 0, 0, 0.8, 0, 0, 0, 0, 0, 0]));
        this.#fallbackMaterialBindGroup = device.createBindGroup({
            label: "Voplay retained 3D fallback material",
            layout: this.#scenePipeline.getBindGroupLayout(1),
            entries: [
                { binding: 0, resource: this.#whiteTextureView },
                { binding: 1, resource: this.#whiteTextureView },
                { binding: 2, resource: this.#whiteTextureView },
                { binding: 3, resource: this.#whiteTextureView },
                { binding: 4, resource: this.#whiteTextureView },
                { binding: 5, resource: this.#sampler },
                { binding: 6, resource: { buffer: this.#fallbackMaterialParameters } },
            ],
        });
        this.#overlayBuffer = device.createBuffer({
            label: "Voplay retained 3D overlay vertices",
            size: this.#overlayCapacity * 24,
            usage: BUFFER_VERTEX | BUFFER_COPY_DST,
        });
    }
    async render(payload, assets) {
        if (this.#closed)
            throw new Error("Voplay retained 3D renderer is closed");
        if (this.#presentError !== null)
            throw this.#presentError;
        await this.#syncAssets(assets);
        const now = performance.now();
        const scene = decodeScene(payload);
        if (this.#scene !== null) {
            this.#previousScene = this.#scene;
            const interval = now - this.#sceneReceivedAt;
            if (interval >= 8 && interval <= 1000) {
                this.#sceneIntervalMillis = Math.max(16, Math.min(250, interval));
            }
        }
        else {
            this.#previousScene = scene;
        }
        this.#scene = scene;
        this.#sceneReceivedAt = now;
        this.#prepareScene();
        const validate = this.#validationFrames > 0;
        await this.#draw(1, validate);
        this.#schedulePresent();
    }
    #prepareScene() {
        if (this.#scene === null || this.#previousScene === null) {
            throw new Error("Voplay retained 3D renderer has no scene");
        }
        const previousInstances = new Map(this.#previousScene.instances.map((instance) => [instance.key, instance]));
        const grouped = new Map();
        for (const instance of this.#scene.instances) {
            if (!this.#meshes.has(instance.mesh))
                continue;
            const key = `${instance.mesh}:${instance.material}`;
            const group = grouped.get(key) ?? {
                mesh: instance.mesh,
                material: instance.material,
                instances: [],
            };
            const previous = previousInstances.get(instance.key);
            group.instances.push({
                previousMatrix: previous?.matrix ?? instance.matrix,
                currentMatrix: instance.matrix,
                color: this.#materials.get(instance.material)?.color ?? [0.72, 0.72, 0.76, 1],
            });
            grouped.set(key, group);
        }
        this.#preparedBatches = [];
        for (const [key, group] of grouped) {
            if (group.instances.length > MAX_INSTANCES) {
                throw new Error("Voplay retained 3D instance capacity exceeded");
            }
            this.#preparedBatches.push({
                mesh: group.mesh,
                material: group.material,
                buffer: this.#instanceBuffer(key, group.instances.length),
                values: new Float32Array(group.instances.length * 20),
                instances: group.instances,
            });
        }
        this.#preparedBatches.sort((left, right) => (this.#materials.get(left.material)?.alphaMode === 3 ? 1 : 0)
            - (this.#materials.get(right.material)?.alphaMode === 3 ? 1 : 0));
        this.#overlayScene = this.#scene.overlays;
        this.#updateOverlay();
    }
    #updateOverlay() {
        const width = Math.max(1, this.#canvas.width);
        const height = Math.max(1, this.#canvas.height);
        const values = decodeOverlays(this.#overlayScene, width, height);
        if (values.length / 6 > MAX_OVERLAY_VERTICES) {
            throw new Error("Voplay retained 3D overlay capacity exceeded");
        }
        const overlay = new Float32Array(values);
        this.#ensureOverlayCapacity(overlay.length / 6);
        if (overlay.length > 0)
            this.#device.queue.writeBuffer(this.#overlayBuffer, 0, overlay);
        this.#overlayVertexCount = overlay.length / 6;
        this.#overlayWidth = width;
        this.#overlayHeight = height;
    }
    async #draw(alpha, validate) {
        if (validate)
            this.#device.pushErrorScope("validation");
        try {
            this.#resize();
            const width = Math.max(1, this.#canvas.width);
            const height = Math.max(1, this.#canvas.height);
            if (width !== this.#overlayWidth || height !== this.#overlayHeight) {
                this.#updateOverlay();
            }
            if (this.#scene === null || this.#previousScene === null) {
                throw new Error("Voplay retained 3D renderer has no scene");
            }
            const camera = {
                ...this.#scene.camera,
                matrix: interpolateMatrix(this.#previousScene.camera.matrix, this.#scene.camera.matrix, alpha),
            };
            const viewProjection = sceneViewProjection(camera, width / height);
            const cameraPosition = [
                camera.matrix[3] / 1000,
                camera.matrix[7] / 1000,
                camera.matrix[11] / 1000,
            ];
            const lightViewProjection = sceneLightViewProjection(cameraPosition);
            const uniform = new Float32Array(48);
            uniform.set(viewProjection, 0);
            uniform.set([-0.36, -0.84, -0.41, 0], 16);
            uniform.set([...cameraPosition, 1], 20);
            uniform.set([...this.#scene.fogColor, 1], 24);
            uniform.set([this.#scene.fogStart, this.#scene.fogEnd, 0, 0], 28);
            uniform.set(lightViewProjection, 32);
            this.#device.queue.writeBuffer(this.#uniform, 0, uniform);
            for (const batch of this.#preparedBatches) {
                for (let index = 0; index < batch.instances.length; index += 1) {
                    writePreparedInstance(batch.values, index * 20, batch.instances[index], alpha);
                }
                this.#device.queue.writeBuffer(batch.buffer, 0, batch.values);
            }
            const encoder = this.#device.createCommandEncoder({
                label: "Voplay retained 3D frame",
            });
            const shadowPass = encoder.beginRenderPass({
                label: "Voplay retained 3D sun shadow pass",
                colorAttachments: [],
                depthStencilAttachment: {
                    view: this.#shadowTexture.createView(),
                    depthClearValue: 1,
                    depthLoadOp: "clear",
                    depthStoreOp: "store",
                },
            });
            shadowPass.setPipeline(this.#shadowPipeline);
            shadowPass.setBindGroup(0, this.#shadowUniformBindGroup);
            for (const batch of this.#preparedBatches) {
                const material = this.#materials.get(batch.material);
                if (material?.alphaMode === 3)
                    continue;
                const mesh = this.#meshes.get(batch.mesh);
                shadowPass.setVertexBuffer(0, mesh.vertex);
                shadowPass.setVertexBuffer(1, batch.buffer);
                shadowPass.setIndexBuffer(mesh.index, "uint32");
                shadowPass.drawIndexed(mesh.indexCount, batch.instances.length, 0, 0, 0);
            }
            shadowPass.end();
            const pass = encoder.beginRenderPass({
                label: "Voplay retained 3D render pass",
                colorAttachments: [{
                        view: this.#colorMsaa.createView(),
                        resolveTarget: this.#context.getCurrentTexture().createView(),
                        clearValue: { r: 0.08, g: 0.48, b: 0.82, a: 1 },
                        loadOp: "clear",
                        storeOp: "store",
                    }],
                depthStencilAttachment: {
                    view: this.#depth.createView(),
                    depthClearValue: 1,
                    depthLoadOp: "clear",
                    depthStoreOp: "store",
                },
            });
            pass.setPipeline(this.#skyPipeline);
            pass.draw(3, 1, 0);
            pass.setBindGroup(0, this.#uniformBindGroup);
            for (const batch of this.#preparedBatches) {
                const mesh = this.#meshes.get(batch.mesh);
                const material = this.#materials.get(batch.material);
                pass.setPipeline(this.#materialPipeline(material));
                pass.setBindGroup(1, this.#materialBindGroup(material));
                pass.setVertexBuffer(0, mesh.vertex);
                pass.setVertexBuffer(1, batch.buffer);
                pass.setIndexBuffer(mesh.index, "uint32");
                pass.drawIndexed(mesh.indexCount, batch.instances.length, 0, 0, 0);
            }
            if (this.#overlayVertexCount > 0) {
                pass.setPipeline(this.#overlayPipeline);
                pass.setVertexBuffer(0, this.#overlayBuffer);
                pass.draw(this.#overlayVertexCount, 1, 0);
            }
            pass.end();
            this.#device.queue.submit([encoder.finish()]);
        }
        catch (error) {
            if (validate)
                await this.#device.popErrorScope();
            throw error;
        }
        if (validate) {
            this.#validationFrames--;
            const validation = await this.#device.popErrorScope();
            if (validation !== null) {
                throw new Error(`Voplay retained WebGPU validation failed: ${validation.message ?? "unknown"}`);
            }
        }
    }
    #sampleAlpha(now) {
        if (this.#scene === null || this.#previousScene === null) {
            throw new Error("Voplay retained 3D renderer has no scene");
        }
        const lead = Math.max(0, Math.min(1, (now - this.#sceneReceivedAt) / this.#sceneIntervalMillis));
        return 1 + lead;
    }
    #schedulePresent() {
        if (this.#animationFrame !== 0 || this.#closed)
            return;
        this.#animationFrame = requestAnimationFrame((now) => this.#present(now));
    }
    #present(now) {
        this.#animationFrame = 0;
        if (this.#closed)
            return;
        this.#schedulePresent();
        if (this.#scene === null || this.#drawing || this.#presentError !== null)
            return;
        if (this.#lastPresentAt !== 0 && now - this.#lastPresentAt < 15)
            return;
        this.#lastPresentAt = now;
        this.#drawing = true;
        const cpuStarted = performance.now();
        void this.#draw(this.#sampleAlpha(now), false).then(() => {
            this.#presentCpuMillis += performance.now() - cpuStarted;
            this.#presentFrames++;
            if (this.#presentStatsStarted === 0)
                this.#presentStatsStarted = now;
            const elapsed = now - this.#presentStatsStarted;
            if (elapsed >= 1000
                && (new URLSearchParams(window.location.search).has("rendererDebug")
                    || new URLSearchParams(window.location.search).has("voplayPresentDebug"))) {
                console.debug(`Voplay retained WebGPU present_fps=${Math.round(this.#presentFrames * 1000 / elapsed)} `
                    + `cpu_ms=${Math.round(this.#presentCpuMillis / this.#presentFrames * 10) / 10}`);
                this.#presentFrames = 0;
                this.#presentCpuMillis = 0;
                this.#presentStatsStarted = now;
            }
        }).catch((error) => {
            this.#presentError = error instanceof Error ? error : new Error(String(error));
        }).finally(() => {
            this.#drawing = false;
        });
    }
    close() {
        if (this.#closed)
            return;
        this.#closed = true;
        if (this.#animationFrame !== 0) {
            cancelAnimationFrame(this.#animationFrame);
            this.#animationFrame = 0;
        }
        for (const mesh of this.#meshes.values()) {
            mesh.vertex.destroy();
            mesh.index.destroy();
            mesh.instance.destroy();
        }
        this.#meshes.clear();
        for (const material of this.#materials.values())
            material.parameters.destroy();
        this.#materials.clear();
        for (const batch of this.#instanceBatches.values())
            batch.buffer.destroy();
        this.#instanceBatches.clear();
        for (const texture of this.#textures.values())
            texture.texture.destroy();
        this.#textures.clear();
        this.#whiteTexture.destroy();
        this.#shadowTexture.destroy();
        this.#fallbackMaterialParameters.destroy();
        this.#overlayBuffer.destroy();
        this.#uniform.destroy();
        this.#depth?.destroy();
        this.#colorMsaa?.destroy();
        this.#context.unconfigure();
        this.#device.destroy();
    }
    #materialBindGroup(material) {
        if (material === undefined)
            return this.#fallbackMaterialBindGroup;
        const textures = material.textures.map((id) => this.#textures.get(id));
        const textureRevisions = textures.map((texture) => texture?.revision ?? 0n);
        if (material.bindGroup !== null
            && textureRevisions.every((revision, index) => material.boundTextureRevisions[index] === revision)) {
            return material.bindGroup;
        }
        material.bindGroup = this.#device.createBindGroup({
            label: "Voplay retained 3D material",
            layout: this.#scenePipeline.getBindGroupLayout(1),
            entries: [
                ...textures.map((texture, binding) => ({
                    binding,
                    resource: texture?.view ?? this.#whiteTextureView,
                })),
                { binding: 5, resource: this.#sampler },
                { binding: 6, resource: { buffer: material.parameters } },
            ],
        });
        material.boundTextureRevisions = textureRevisions;
        return material.bindGroup;
    }
    #materialPipeline(material) {
        return this.#scenePipeline;
    }
    async #syncAssets(assets) {
        for (const asset of assets) {
            if (asset.kind === 2) {
                const current = this.#meshes.get(asset.asset);
                if (current?.revision === asset.revision)
                    continue;
                const decoded = decodeMeshArtifact(asset.bytes, asset.asset);
                const vertex = this.#device.createBuffer({
                    label: `Voplay retained mesh ${asset.asset} vertices`,
                    size: Math.max(4, decoded.vertices.byteLength),
                    usage: BUFFER_VERTEX | BUFFER_COPY_DST,
                });
                const index = this.#device.createBuffer({
                    label: `Voplay retained mesh ${asset.asset} indices`,
                    size: Math.max(4, decoded.indices.byteLength),
                    usage: BUFFER_INDEX | BUFFER_COPY_DST,
                });
                this.#device.queue.writeBuffer(vertex, 0, decoded.vertices);
                this.#device.queue.writeBuffer(index, 0, decoded.indices);
                const instance = this.#device.createBuffer({
                    label: `Voplay retained mesh ${asset.asset} instances`,
                    size: 20 * 4,
                    usage: BUFFER_VERTEX | BUFFER_COPY_DST,
                });
                current?.vertex.destroy();
                current?.index.destroy();
                current?.instance.destroy();
                this.#meshes.set(asset.asset, {
                    revision: asset.revision,
                    vertex,
                    index,
                    indexCount: decoded.indices.length,
                    instance,
                    instanceCapacity: 1,
                });
            }
            else if (asset.kind === 3) {
                const current = this.#materials.get(asset.asset);
                if (current?.revision === asset.revision)
                    continue;
                const decoded = decodeMaterial(asset.bytes, asset.asset);
                const parameters = this.#device.createBuffer({
                    label: `Voplay retained material ${asset.asset} parameters`,
                    size: 48,
                    usage: BUFFER_UNIFORM | BUFFER_COPY_DST,
                });
                this.#device.queue.writeBuffer(parameters, 0, decoded.parameters);
                current?.parameters.destroy();
                this.#materials.set(asset.asset, {
                    revision: asset.revision,
                    color: decoded.color,
                    textures: decoded.textures,
                    alphaMode: decoded.alphaMode,
                    doubleSided: decoded.doubleSided,
                    parameters,
                    bindGroup: null,
                    boundTextureRevisions: [],
                });
            }
            else if (asset.kind === 6) {
                const current = this.#textures.get(asset.asset);
                if (current?.revision === asset.revision)
                    continue;
                const bitmap = await createImageBitmap(new Blob([asset.bytes.slice()], { type: "image/png" }));
                try {
                    if (bitmap.width === 0 || bitmap.height === 0) {
                        throw new Error("Voplay retained texture has invalid dimensions");
                    }
                    const texture = this.#device.createTexture({
                        label: `Voplay retained texture ${asset.asset}`,
                        size: [bitmap.width, bitmap.height, 1],
                        format: "rgba8unorm",
                        usage: TEXTURE_BINDING | TEXTURE_COPY_DST | TEXTURE_RENDER_ATTACHMENT,
                    });
                    this.#device.queue.copyExternalImageToTexture({ source: bitmap }, { texture }, [bitmap.width, bitmap.height, 1]);
                    current?.texture.destroy();
                    this.#textures.set(asset.asset, {
                        revision: asset.revision,
                        texture,
                        view: texture.createView(),
                    });
                }
                finally {
                    bitmap.close();
                }
            }
        }
    }
    #resize() {
        const width = Math.max(1, this.#canvas.width);
        const height = Math.max(1, this.#canvas.height);
        if (width === this.#width && height === this.#height)
            return;
        this.#width = width;
        this.#height = height;
        this.#context.configure({
            device: this.#device,
            format: this.#format,
            alphaMode: "opaque",
        });
        this.#depth?.destroy();
        this.#colorMsaa?.destroy();
        this.#colorMsaa = this.#device.createTexture({
            label: "Voplay retained 3D multisampled color",
            size: [width, height, 1],
            sampleCount: PRESENT_SAMPLE_COUNT,
            format: this.#format,
            usage: TEXTURE_RENDER_ATTACHMENT,
        });
        this.#depth = this.#device.createTexture({
            label: "Voplay retained 3D depth",
            size: [width, height, 1],
            format: "depth24plus",
            sampleCount: PRESENT_SAMPLE_COUNT,
            usage: TEXTURE_RENDER_ATTACHMENT,
        });
    }
    #instanceBuffer(key, count) {
        const current = this.#instanceBatches.get(key);
        if (current !== undefined && current.capacity >= count)
            return current.buffer;
        let capacity = current?.capacity ?? 1;
        while (capacity < count)
            capacity *= 2;
        current?.buffer.destroy();
        const buffer = this.#device.createBuffer({
            label: `Voplay retained 3D instances ${key}`,
            size: capacity * 20 * 4,
            usage: BUFFER_VERTEX | BUFFER_COPY_DST,
        });
        this.#instanceBatches.set(key, { buffer, capacity });
        return buffer;
    }
    #ensureOverlayCapacity(count) {
        if (this.#overlayCapacity >= count)
            return;
        let capacity = this.#overlayCapacity;
        while (capacity < count)
            capacity *= 2;
        this.#overlayBuffer.destroy();
        this.#overlayBuffer = this.#device.createBuffer({
            label: "Voplay retained 3D overlay vertices",
            size: capacity * 24,
            usage: BUFFER_VERTEX | BUFFER_COPY_DST,
        });
        this.#overlayCapacity = capacity;
    }
}
function createScenePipeline(device, format) {
    const module = device.createShaderModule({
        label: "Voplay retained 3D shader",
        code: `
struct Scene {
  view_projection: mat4x4<f32>,
  light_direction: vec4<f32>,
  camera_position: vec4<f32>,
  fog_color: vec4<f32>,
  fog_range: vec4<f32>,
  light_view_projection: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> scene: Scene;
@group(0) @binding(1) var sun_shadow: texture_depth_2d;
@group(0) @binding(2) var sun_shadow_sampler: sampler_comparison;
@group(1) @binding(0) var material_texture_0: texture_2d<f32>;
@group(1) @binding(1) var material_texture_1: texture_2d<f32>;
@group(1) @binding(2) var material_texture_2: texture_2d<f32>;
@group(1) @binding(3) var material_texture_3: texture_2d<f32>;
@group(1) @binding(4) var material_texture_4: texture_2d<f32>;
@group(1) @binding(5) var material_sampler: sampler;

struct Material {
  flags: vec4<f32>,
  factors: vec4<f32>,
  emissive: vec4<f32>,
};
@group(1) @binding(6) var<uniform> material: Material;

struct VertexIn {
  @location(0) position: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) texcoord: vec2<f32>,
  @location(3) model_0: vec4<f32>,
  @location(4) model_1: vec4<f32>,
  @location(5) model_2: vec4<f32>,
  @location(6) model_3: vec4<f32>,
  @location(7) color: vec4<f32>,
};

struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) world_position: vec3<f32>,
  @location(1) world_normal: vec3<f32>,
  @location(2) color: vec4<f32>,
  @location(3) texcoord: vec2<f32>,
  @location(4) shadow_position: vec4<f32>,
};

@vertex fn vertex_main(input: VertexIn) -> VertexOut {
  let model = mat4x4<f32>(input.model_0, input.model_1, input.model_2, input.model_3);
  let world = model * vec4<f32>(input.position, 1.0);
  var output: VertexOut;
  output.position = scene.view_projection * world;
  output.world_position = world.xyz;
  output.world_normal = normalize((model * vec4<f32>(input.normal, 0.0)).xyz);
  output.color = input.color;
  output.texcoord = input.texcoord;
  output.shadow_position = scene.light_view_projection * world;
  return output;
}

@fragment fn fragment_main(input: VertexOut) -> @location(0) vec4<f32> {
  var albedo = textureSample(material_texture_0, material_sampler, input.texcoord) * input.color;
  if (material.flags.x > 0.5) {
    let control_uv = vec2<f32>(
      input.world_position.x / 1127.0 + 0.5,
      0.5 - input.world_position.z / 1127.0
    );
    let raw_weights = max(
      textureSample(material_texture_0, material_sampler, control_uv),
      vec4<f32>(0.0001)
    );
    let weights = raw_weights / dot(raw_weights, vec4<f32>(1.0));
    albedo = (
      textureSample(material_texture_1, material_sampler, input.texcoord * 1.12) * weights.x
      + textureSample(material_texture_2, material_sampler, input.texcoord * 0.94) * weights.y
      + textureSample(material_texture_3, material_sampler, input.texcoord * 0.72) * weights.z
      + textureSample(material_texture_4, material_sampler, input.texcoord * 0.58) * weights.w
    ) * input.color;
  }
  if (material.flags.y > 1.5 && material.flags.y < 2.5 && albedo.a < material.flags.z) {
    discard;
  }
  let normal = normalize(input.world_normal);
  let light = normalize(-scene.light_direction.xyz);
  let view = normalize(scene.camera_position.xyz - input.world_position);
  let half_vector = normalize(light + view);
  let shadow_ndc = input.shadow_position.xyz / input.shadow_position.w;
  let shadow_uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, 0.5 - shadow_ndc.y * 0.5);
  let shadow_inside = all(shadow_uv >= vec2<f32>(0.002))
    && all(shadow_uv <= vec2<f32>(0.998))
    && shadow_ndc.z >= 0.0
    && shadow_ndc.z <= 1.0;
  let sampled = textureSampleCompare(
    sun_shadow,
    sun_shadow_sampler,
    clamp(shadow_uv, vec2<f32>(0.002), vec2<f32>(0.998)),
    shadow_ndc.z - 0.0018
  );
  let sun_visibility = select(1.0, mix(0.38, 1.0, sampled), shadow_inside);
  let diffuse = max(dot(normal, light), 0.0) * sun_visibility;
  let hemi = normal.y * 0.18 + 0.18;
  let metallic = material.factors.x;
  let roughness = max(material.factors.y, 0.04);
  let specular_power = mix(72.0, 5.0, roughness);
  let specular = pow(max(dot(normal, half_vector), 0.0), specular_power) * diffuse;
  let fresnel = mix(vec3<f32>(0.04), albedo.rgb, metallic);
  var lit = albedo.rgb * (0.32 + diffuse * 0.76 + hemi) * (1.0 - metallic * 0.34)
    + fresnel * specular * (1.0 - roughness * 0.55)
    + material.emissive.rgb;
  if (material.flags.w > 0.5) {
    lit = albedo.rgb + material.emissive.rgb;
  }
  let distance_to_camera = distance(input.world_position, scene.camera_position.xyz);
  let fog = smoothstep(scene.fog_range.x, scene.fog_range.y, distance_to_camera);
  return vec4<f32>(mix(lit, scene.fog_color.rgb, fog), albedo.a);
}`,
    });
    return device.createRenderPipeline({
        label: "Voplay retained 3D pipeline",
        layout: "auto",
        vertex: {
            module,
            entryPoint: "vertex_main",
            buffers: [
                {
                    arrayStride: 32,
                    attributes: [
                        { shaderLocation: 0, offset: 0, format: "float32x3" },
                        { shaderLocation: 1, offset: 12, format: "float32x3" },
                        { shaderLocation: 2, offset: 24, format: "float32x2" },
                    ],
                },
                {
                    arrayStride: 80,
                    stepMode: "instance",
                    attributes: [
                        { shaderLocation: 3, offset: 0, format: "float32x4" },
                        { shaderLocation: 4, offset: 16, format: "float32x4" },
                        { shaderLocation: 5, offset: 32, format: "float32x4" },
                        { shaderLocation: 6, offset: 48, format: "float32x4" },
                        { shaderLocation: 7, offset: 64, format: "float32x4" },
                    ],
                },
            ],
        },
        fragment: {
            module,
            entryPoint: "fragment_main",
            targets: [{
                    format,
                    blend: {
                        color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
                        alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
                    },
                }],
        },
        primitive: {
            topology: "triangle-list",
            cullMode: "none",
            frontFace: "ccw",
        },
        depthStencil: {
            format: "depth24plus",
            depthWriteEnabled: true,
            depthCompare: "less",
        },
        multisample: { count: PRESENT_SAMPLE_COUNT },
    });
}
function createSkyPipeline(device, format) {
    const module = device.createShaderModule({
        label: "Voplay retained 3D atmosphere shader",
        code: `
struct SkyOut {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex fn vertex_main(@builtin(vertex_index) vertex: u32) -> SkyOut {
  let positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0)
  );
  var output: SkyOut;
  output.position = vec4<f32>(positions[vertex], 1.0, 1.0);
  output.uv = positions[vertex] * 0.5 + 0.5;
  return output;
}

fn cloud_blob(uv: vec2<f32>, center: vec2<f32>, radius: vec2<f32>) -> f32 {
  let delta = (uv - center) / radius;
  return 1.0 - smoothstep(0.52, 1.0, dot(delta, delta));
}

@fragment fn fragment_main(input: SkyOut) -> @location(0) vec4<f32> {
  let horizon = vec3<f32>(0.58, 0.83, 0.94);
  let zenith = vec3<f32>(0.055, 0.36, 0.76);
  let vertical = pow(clamp(input.uv.y, 0.0, 1.0), 0.72);
  var color = mix(horizon, zenith, vertical);
  let sun_delta = input.uv - vec2<f32>(0.78, 0.79);
  let sun = 1.0 - smoothstep(0.012, 0.065, length(sun_delta));
  color += vec3<f32>(1.0, 0.72, 0.34) * sun * 0.62;
  var clouds = 0.0;
  clouds = max(clouds, cloud_blob(input.uv, vec2<f32>(0.12, 0.73), vec2<f32>(0.12, 0.045)));
  clouds = max(clouds, cloud_blob(input.uv, vec2<f32>(0.19, 0.75), vec2<f32>(0.095, 0.060)));
  clouds = max(clouds, cloud_blob(input.uv, vec2<f32>(0.47, 0.83), vec2<f32>(0.15, 0.052)));
  clouds = max(clouds, cloud_blob(input.uv, vec2<f32>(0.56, 0.85), vec2<f32>(0.085, 0.065)));
  clouds = max(clouds, cloud_blob(input.uv, vec2<f32>(0.88, 0.68), vec2<f32>(0.13, 0.046)));
  let cloud_light = vec3<f32>(1.0, 0.99, 0.94);
  color = mix(color, cloud_light, clouds * 0.88);
  return vec4<f32>(color, 1.0);
}`,
    });
    return device.createRenderPipeline({
        label: "Voplay retained 3D atmosphere pipeline",
        layout: "auto",
        vertex: { module, entryPoint: "vertex_main" },
        fragment: { module, entryPoint: "fragment_main", targets: [{ format }] },
        primitive: { topology: "triangle-list" },
        depthStencil: {
            format: "depth24plus",
            depthWriteEnabled: false,
            depthCompare: "always",
        },
        multisample: { count: PRESENT_SAMPLE_COUNT },
    });
}
function createShadowPipeline(device) {
    const module = device.createShaderModule({
        label: "Voplay retained 3D shadow shader",
        code: `
struct Scene {
  view_projection: mat4x4<f32>,
  light_direction: vec4<f32>,
  camera_position: vec4<f32>,
  fog_color: vec4<f32>,
  fog_range: vec4<f32>,
  light_view_projection: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> scene: Scene;

struct VertexIn {
  @location(0) position: vec3<f32>,
  @location(3) model_0: vec4<f32>,
  @location(4) model_1: vec4<f32>,
  @location(5) model_2: vec4<f32>,
  @location(6) model_3: vec4<f32>,
};

@vertex fn vertex_main(input: VertexIn) -> @builtin(position) vec4<f32> {
  let model = mat4x4<f32>(input.model_0, input.model_1, input.model_2, input.model_3);
  return scene.light_view_projection * model * vec4<f32>(input.position, 1.0);
}`,
    });
    return device.createRenderPipeline({
        label: "Voplay retained 3D shadow pipeline",
        layout: "auto",
        vertex: {
            module,
            entryPoint: "vertex_main",
            buffers: [
                {
                    arrayStride: 32,
                    attributes: [{ shaderLocation: 0, offset: 0, format: "float32x3" }],
                },
                {
                    arrayStride: 80,
                    stepMode: "instance",
                    attributes: [
                        { shaderLocation: 3, offset: 0, format: "float32x4" },
                        { shaderLocation: 4, offset: 16, format: "float32x4" },
                        { shaderLocation: 5, offset: 32, format: "float32x4" },
                        { shaderLocation: 6, offset: 48, format: "float32x4" },
                    ],
                },
            ],
        },
        primitive: { topology: "triangle-list", cullMode: "back", frontFace: "ccw" },
        depthStencil: {
            format: "depth24plus",
            depthWriteEnabled: true,
            depthCompare: "less",
            depthBias: 2,
            depthBiasSlopeScale: 1.5,
            depthBiasClamp: 0,
        },
    });
}
function createOverlayPipeline(device, format) {
    const module = device.createShaderModule({
        label: "Voplay retained 3D overlay shader",
        code: `
struct OverlayIn {
  @location(0) position: vec2<f32>,
  @location(1) color: vec4<f32>,
};
struct OverlayOut {
  @builtin(position) position: vec4<f32>,
  @location(0) color: vec4<f32>,
};
@vertex fn vertex_main(input: OverlayIn) -> OverlayOut {
  var output: OverlayOut;
  output.position = vec4<f32>(input.position, 0.0, 1.0);
  output.color = input.color;
  return output;
}
@fragment fn fragment_main(input: OverlayOut) -> @location(0) vec4<f32> {
  return input.color;
}`,
    });
    return device.createRenderPipeline({
        label: "Voplay retained 3D overlay pipeline",
        layout: "auto",
        vertex: {
            module,
            entryPoint: "vertex_main",
            buffers: [{
                    arrayStride: 24,
                    attributes: [
                        { shaderLocation: 0, offset: 0, format: "float32x2" },
                        { shaderLocation: 1, offset: 8, format: "float32x4" },
                    ],
                }],
        },
        fragment: {
            module,
            entryPoint: "fragment_main",
            targets: [{
                    format,
                    blend: {
                        color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
                        alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
                    },
                }],
        },
        primitive: { topology: "triangle-list" },
        depthStencil: {
            format: "depth24plus",
            depthWriteEnabled: false,
            depthCompare: "always",
        },
        multisample: { count: PRESENT_SAMPLE_COUNT },
    });
}
function decodeScene(payload) {
    if (payload.byteLength < 4)
        throw new Error("truncated Voplay retained 3D snapshot");
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const count = view.getUint32(0, true);
    if (count > 1_000_000)
        throw new Error("Voplay retained 3D snapshot capacity exceeded");
    const instances = [];
    const overlays = [];
    let camera = null;
    let fogColor = [0.52, 0.78, 0.9];
    let fogStart = 220;
    let fogEnd = 1100;
    let offset = 4;
    for (let index = 0; index < count; index += 1) {
        requireBytes(payload, offset, 12);
        const entityKey = `${view.getUint32(offset, true)}:${view.getUint32(offset + 4, true)}`;
        const length = view.getUint32(offset + 8, true);
        offset += 12;
        requireBytes(payload, offset, length);
        const object = payload.subarray(offset, offset + length);
        offset += length;
        if (isOverlay(object)) {
            overlays.push(object.slice());
            continue;
        }
        if (object.byteLength < 272
            || object[0] !== 0x56
            || object[1] !== 0x52
            || object[2] !== 0x57
            || object[3] !== 0x31) {
            continue;
        }
        const objectView = new DataView(object.buffer, object.byteOffset, object.byteLength);
        const matrix = Array.from({ length: 12 }, (_, matrixIndex) => Number(objectView.getBigInt64(124 + matrixIndex * 8, true)));
        const componentCount = objectView.getUint32(268, true);
        let componentOffset = 272;
        for (let componentIndex = 0; componentIndex < componentCount; componentIndex += 1) {
            requireBytes(object, componentOffset, 16);
            const kind = objectView.getUint32(componentOffset, true);
            const componentLength = objectView.getUint32(componentOffset + 12, true);
            componentOffset += 16;
            requireBytes(object, componentOffset, componentLength);
            const component = object.subarray(componentOffset, componentOffset + componentLength);
            componentOffset += componentLength;
            if (kind === 1
                && component.byteLength >= 40
                && component[0] === 0x56
                && component[1] === 0x4d
                && component[2] === 0x33
                && component[3] === 0x31) {
                const componentView = new DataView(component.buffer, component.byteOffset, component.byteLength);
                const material = componentView.getBigUint64(4, true);
                const mesh = componentView.getBigUint64(24, true);
                instances.push({ key: entityKey, mesh, material, matrix });
            }
            else if (kind === 2
                && component.byteLength >= 21
                && component[0] === 0x56
                && component[1] === 0x43
                && component[2] === 0x33
                && component[3] === 0x31) {
                const componentView = new DataView(component.buffer, component.byteOffset, component.byteLength);
                camera = {
                    matrix,
                    verticalFovDegrees: componentView.getUint32(5, true) / 1000,
                };
            }
            else if (kind === 10
                && component.byteLength >= 38
                && component[0] === 0x56
                && component[1] === 0x45
                && component[2] === 0x33
                && component[3] === 0x31) {
                const componentView = new DataView(component.buffer, component.byteOffset, component.byteLength);
                fogColor = [
                    componentView.getUint16(6, true) / 65535,
                    componentView.getUint16(8, true) / 65535,
                    componentView.getUint16(10, true) / 65535,
                ];
                fogStart = Number(componentView.getBigUint64(12, true)) / 1000;
                fogEnd = Number(componentView.getBigUint64(20, true)) / 1000;
            }
            else if (kind === 20
                && component.byteLength >= 136
                && component[0] === 0x56
                && component[1] === 0x50
                && component[2] === 0x33
                && component[3] === 0x31) {
                instances.push(...decodeParticleInstances(component, entityKey));
            }
            else if (isOverlay(component)) {
                overlays.push(component.slice());
            }
        }
    }
    if (offset !== payload.byteLength)
        throw new Error("Voplay retained 3D snapshot trailing bytes");
    if (camera === null)
        throw new Error("Voplay retained 3D snapshot has no camera");
    return { instances, camera, overlays, fogColor, fogStart, fogEnd };
}
function decodeParticleInstances(bytes, entityKey) {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const seed = view.getBigUint64(12, true);
    const mesh = view.getBigUint64(20, true);
    const material = view.getBigUint64(28, true);
    const maxParticles = view.getUint32(36, true);
    const spawnPerTick = view.getUint32(40, true);
    const lifetimeTicks = view.getUint32(44, true);
    const position = [
        Number(view.getBigInt64(48, true)),
        Number(view.getBigInt64(56, true)),
        Number(view.getBigInt64(64, true)),
    ];
    const velocityMin = [
        Number(view.getBigInt64(72, true)),
        Number(view.getBigInt64(80, true)),
        Number(view.getBigInt64(88, true)),
    ];
    const velocityMax = [
        Number(view.getBigInt64(96, true)),
        Number(view.getBigInt64(104, true)),
        Number(view.getBigInt64(112, true)),
    ];
    const startScale = view.getUint32(120, true);
    const endScale = view.getUint32(124, true);
    const count = Math.min(maxParticles, spawnPerTick * Math.min(lifetimeTicks, 8));
    const output = [];
    for (let index = 0; index < count; index += 1) {
        const ageTicks = index / Math.max(1, spawnPerTick);
        const life = Math.min(1, ageTicks / Math.max(1, lifetimeTicks));
        const randomX = particleRandom(seed, index, 1);
        const randomY = particleRandom(seed, index, 2);
        const randomZ = particleRandom(seed, index, 3);
        const velocity = [
            velocityMin[0] + (velocityMax[0] - velocityMin[0]) * randomX,
            velocityMin[1] + (velocityMax[1] - velocityMin[1]) * randomY,
            velocityMin[2] + (velocityMax[2] - velocityMin[2]) * randomZ,
        ];
        const seconds = ageTicks * 0.1;
        const scale = startScale + (endScale - startScale) * life;
        const x = position[0] + velocity[0] * seconds;
        const y = position[1] + velocity[1] * seconds;
        const z = position[2] + velocity[2] * seconds;
        output.push({
            key: `${entityKey}:particle:${index}`,
            mesh,
            material,
            matrix: [
                scale, 0, 0, x,
                0, scale, 0, y,
                0, 0, scale, z,
            ],
        });
    }
    return output;
}
function particleRandom(seed, index, channel) {
    let value = Number((seed + BigInt(index * 1103515245 + channel * 2654435761)) & 0xffffffffn) >>> 0;
    value ^= value << 13;
    value ^= value >>> 17;
    value ^= value << 5;
    return (value >>> 0) / 0xffffffff;
}
function interpolateMatrix(previous, current, alpha) {
    return current.map((value, index) => previous[index] + (value - previous[index]) * alpha);
}
function decodeMeshArtifact(bytes, expectedId) {
    if (bytes.byteLength < 21
        || bytes[0] !== 0x56
        || bytes[1] !== 0x4d
        || bytes[2] !== 0x47
        || bytes[3] !== 0x31) {
        throw new Error("invalid Voplay retained mesh artifact");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const id = view.getBigUint64(4, true);
    const vertexCount = view.getUint32(12, true);
    const indexCount = view.getUint32(16, true);
    const vertexBytes = vertexCount * 32;
    const indexBytes = indexCount * 4;
    if (id !== expectedId
        || bytes[20] !== 0
        || vertexCount === 0
        || indexCount === 0
        || bytes.byteLength !== 21 + vertexBytes + indexBytes) {
        throw new Error("invalid Voplay retained mesh artifact layout");
    }
    const vertices = new Float32Array(vertexCount * 8);
    let offset = 21;
    for (let index = 0; index < vertices.length; index += 1) {
        vertices[index] = view.getFloat32(offset, true);
        offset += 4;
    }
    const indices = new Uint32Array(indexCount);
    for (let index = 0; index < indexCount; index += 1) {
        indices[index] = view.getUint32(offset, true);
        offset += 4;
    }
    return { vertices, indices };
}
function decodeMaterial(bytes, expectedId) {
    if (bytes.byteLength < 91
        || bytes[0] !== 0x56
        || bytes[1] !== 0x41
        || bytes[2] !== 0x33
        || bytes[3] !== 0x31) {
        throw new Error("invalid Voplay retained material");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (view.getBigUint64(4, true) !== expectedId) {
        throw new Error("Voplay retained material identity mismatch");
    }
    const color = [
        view.getUint16(31, true) / 65535,
        view.getUint16(33, true) / 65535,
        view.getUint16(35, true) / 65535,
        view.getUint16(37, true) / 65535,
    ];
    const textures = Array.from({ length: 5 }, (_, index) => view.getBigUint64(51 + index * 8, true));
    const terrainSplat = textures.every((texture) => texture !== 0n) ? 1 : 0;
    const alphaMode = bytes[28];
    return {
        color,
        textures,
        alphaMode,
        doubleSided: bytes[29] !== 0,
        parameters: new Float32Array([
            terrainSplat,
            alphaMode,
            view.getUint16(49, true) / 65535,
            bytes[30] === 0 ? 0 : 1,
            view.getUint16(39, true) / 65535,
            view.getUint16(41, true) / 65535,
            0,
            0,
            view.getUint16(43, true) / 65535,
            view.getUint16(45, true) / 65535,
            view.getUint16(47, true) / 65535,
            0,
        ]),
    };
}
function writePreparedInstance(output, offset, instance, alpha) {
    const previous = instance.previousMatrix;
    const current = instance.currentMatrix;
    output[offset] = (previous[0] + (current[0] - previous[0]) * alpha) / 1000;
    output[offset + 1] = (previous[4] + (current[4] - previous[4]) * alpha) / 1000;
    output[offset + 2] = (previous[8] + (current[8] - previous[8]) * alpha) / 1000;
    output[offset + 3] = 0;
    output[offset + 4] = (previous[1] + (current[1] - previous[1]) * alpha) / 1000;
    output[offset + 5] = (previous[5] + (current[5] - previous[5]) * alpha) / 1000;
    output[offset + 6] = (previous[9] + (current[9] - previous[9]) * alpha) / 1000;
    output[offset + 7] = 0;
    output[offset + 8] = (previous[2] + (current[2] - previous[2]) * alpha) / 1000;
    output[offset + 9] = (previous[6] + (current[6] - previous[6]) * alpha) / 1000;
    output[offset + 10] = (previous[10] + (current[10] - previous[10]) * alpha) / 1000;
    output[offset + 11] = 0;
    output[offset + 12] = (previous[3] + (current[3] - previous[3]) * alpha) / 1000;
    output[offset + 13] = (previous[7] + (current[7] - previous[7]) * alpha) / 1000;
    output[offset + 14] = (previous[11] + (current[11] - previous[11]) * alpha) / 1000;
    output[offset + 15] = 1;
    output[offset + 16] = instance.color[0];
    output[offset + 17] = instance.color[1];
    output[offset + 18] = instance.color[2];
    output[offset + 19] = instance.color[3];
}
function sceneViewProjection(camera, aspect) {
    const matrix = camera.matrix;
    const right = [matrix[0] / 1000, matrix[4] / 1000, matrix[8] / 1000];
    const up = [matrix[1] / 1000, matrix[5] / 1000, matrix[9] / 1000];
    const back = [matrix[2] / 1000, matrix[6] / 1000, matrix[10] / 1000];
    const position = [matrix[3] / 1000, matrix[7] / 1000, matrix[11] / 1000];
    const view = new Float32Array([
        right[0], up[0], back[0], 0,
        right[1], up[1], back[1], 0,
        right[2], up[2], back[2], 0,
        -dot3(right, position), -dot3(up, position), -dot3(back, position), 1,
    ]);
    const near = 0.08;
    const far = 2400;
    const f = 1 / Math.tan(camera.verticalFovDegrees * Math.PI / 360);
    const projection = new Float32Array([
        f / Math.max(0.01, aspect), 0, 0, 0,
        0, f, 0, 0,
        0, 0, far / (near - far), -1,
        0, 0, far * near / (near - far), 0,
    ]);
    return multiplyMat4(projection, view);
}
function sceneLightViewProjection(cameraPosition) {
    const forward = normalizeVector3([-0.36, -0.84, -0.41]);
    const center = [cameraPosition[0], 12, cameraPosition[2]];
    const eye = [
        center[0] - forward[0] * 320,
        center[1] - forward[1] * 320,
        center[2] - forward[2] * 320,
    ];
    const back = [-forward[0], -forward[1], -forward[2]];
    const right = normalizeVector3(crossVector3([0, 1, 0], back));
    const up = crossVector3(back, right);
    const view = new Float32Array([
        right[0], up[0], back[0], 0,
        right[1], up[1], back[1], 0,
        right[2], up[2], back[2], 0,
        -dot3(right, eye), -dot3(up, eye), -dot3(back, eye), 1,
    ]);
    const extent = 190;
    const near = 0.1;
    const far = 720;
    const projection = new Float32Array([
        1 / extent, 0, 0, 0,
        0, 1 / extent, 0, 0,
        0, 0, 1 / (near - far), 0,
        0, 0, near / (near - far), 1,
    ]);
    return multiplyMat4(projection, view);
}
function multiplyMat4(left, right) {
    const output = new Float32Array(16);
    for (let column = 0; column < 4; column += 1) {
        for (let row = 0; row < 4; row += 1) {
            let value = 0;
            for (let index = 0; index < 4; index += 1) {
                value += left[index * 4 + row] * right[column * 4 + index];
            }
            output[column * 4 + row] = value;
        }
    }
    return output;
}
function crossVector3(left, right) {
    return [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ];
}
function normalizeVector3(value) {
    const length = Math.hypot(value[0], value[1], value[2]);
    return length > 0.000001
        ? [value[0] / length, value[1] / length, value[2] / length]
        : [0, 1, 0];
}
function dot3(left, right) {
    return left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
}
function decodeOverlays(overlays, width, height) {
    const vertices = [];
    for (const bytes of overlays) {
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        if (bytes[1] === 0x48 && bytes.byteLength >= 81) {
            const x = Number(view.getBigInt64(13, true)) / 1000;
            const y = Number(view.getBigInt64(21, true)) / 1000;
            const shapeWidth = Number(view.getBigInt64(29, true)) / 1000;
            const shapeHeight = Number(view.getBigInt64(37, true)) / 1000;
            if (bytes[56] !== 0) {
                pushOverlayRect(vertices, width, height, x, y, shapeWidth, shapeHeight, [
                    bytes[53] / 255,
                    bytes[54] / 255,
                    bytes[55] / 255,
                    bytes[56] / 255,
                ]);
            }
            continue;
        }
        if (bytes[1] === 0x54 && bytes.byteLength >= 72) {
            const x = Number(view.getBigInt64(20, true)) / 1000;
            const baseline = Number(view.getBigInt64(28, true)) / 1000;
            const size = Math.max(7, view.getUint32(36, true) / 1000);
            const length = view.getUint32(68, true);
            if (length > bytes.byteLength - 72)
                continue;
            const text = new TextDecoder().decode(bytes.subarray(72, 72 + length)).toUpperCase();
            pushOverlayText(vertices, width, height, x, baseline, size, text, [
                bytes[64] / 255,
                bytes[65] / 255,
                bytes[66] / 255,
                bytes[67] / 255,
            ]);
        }
    }
    return vertices;
}
function pushOverlayRect(output, width, height, x, y, rectWidth, rectHeight, color) {
    const left = x / width * 2 - 1;
    const right = (x + rectWidth) / width * 2 - 1;
    const top = 1 - y / height * 2;
    const bottom = 1 - (y + rectHeight) / height * 2;
    pushOverlayVertex(output, left, top, color);
    pushOverlayVertex(output, right, top, color);
    pushOverlayVertex(output, left, bottom, color);
    pushOverlayVertex(output, left, bottom, color);
    pushOverlayVertex(output, right, top, color);
    pushOverlayVertex(output, right, bottom, color);
}
function pushOverlayText(output, width, height, x, baseline, size, text, color) {
    const pixel = size / 7;
    let cursor = x;
    for (const character of text) {
        const rows = FONT_5X7[character] ?? FONT_5X7["?"];
        for (let row = 0; row < 7; row += 1) {
            for (let column = 0; column < 5; column += 1) {
                if ((rows[row] & (1 << (4 - column))) === 0)
                    continue;
                pushOverlayRect(output, width, height, cursor + column * pixel, baseline - size + row * pixel, pixel * 0.86, pixel * 0.86, color);
            }
        }
        cursor += pixel * 6;
    }
}
function pushOverlayVertex(output, x, y, color) {
    output.push(x, y, ...color);
}
const FONT_5X7 = {
    " ": [0, 0, 0, 0, 0, 0, 0],
    "?": [14, 17, 1, 2, 4, 0, 4],
    "-": [0, 0, 0, 31, 0, 0, 0],
    "/": [1, 2, 2, 4, 8, 8, 16],
    "0": [14, 17, 19, 21, 25, 17, 14],
    "1": [4, 12, 4, 4, 4, 4, 14],
    "2": [14, 17, 1, 2, 4, 8, 31],
    "3": [30, 1, 1, 14, 1, 1, 30],
    "4": [2, 6, 10, 18, 31, 2, 2],
    "5": [31, 16, 16, 30, 1, 1, 30],
    "6": [14, 16, 16, 30, 17, 17, 14],
    "7": [31, 1, 2, 4, 8, 8, 8],
    "8": [14, 17, 17, 14, 17, 17, 14],
    "9": [14, 17, 17, 15, 1, 1, 14],
    "A": [14, 17, 17, 31, 17, 17, 17],
    "B": [30, 17, 17, 30, 17, 17, 30],
    "C": [14, 17, 16, 16, 16, 17, 14],
    "D": [30, 17, 17, 17, 17, 17, 30],
    "E": [31, 16, 16, 30, 16, 16, 31],
    "F": [31, 16, 16, 30, 16, 16, 16],
    "G": [14, 17, 16, 23, 17, 17, 15],
    "H": [17, 17, 17, 31, 17, 17, 17],
    "I": [14, 4, 4, 4, 4, 4, 14],
    "J": [7, 2, 2, 2, 2, 18, 12],
    "K": [17, 18, 20, 24, 20, 18, 17],
    "L": [16, 16, 16, 16, 16, 16, 31],
    "M": [17, 27, 21, 21, 17, 17, 17],
    "N": [17, 25, 21, 19, 17, 17, 17],
    "O": [14, 17, 17, 17, 17, 17, 14],
    "P": [30, 17, 17, 30, 16, 16, 16],
    "Q": [14, 17, 17, 17, 21, 18, 13],
    "R": [30, 17, 17, 30, 20, 18, 17],
    "S": [15, 16, 16, 14, 1, 1, 30],
    "T": [31, 4, 4, 4, 4, 4, 4],
    "U": [17, 17, 17, 17, 17, 17, 14],
    "V": [17, 17, 17, 17, 17, 10, 4],
    "W": [17, 17, 17, 21, 21, 21, 10],
    "X": [17, 17, 10, 4, 10, 17, 17],
    "Y": [17, 17, 10, 4, 4, 4, 4],
    "Z": [31, 1, 2, 4, 8, 16, 31],
};
function isOverlay(bytes) {
    return bytes.byteLength >= 4
        && bytes[0] === 0x56
        && bytes[2] === 0x32
        && bytes[3] === 0x31;
}
function requireBytes(bytes, offset, length) {
    if (length < 0
        || !Number.isSafeInteger(offset + length)
        || offset < 0
        || offset + length > bytes.byteLength) {
        throw new Error("truncated Voplay retained 3D payload");
    }
}
