const BUFFER_COPY_DST = 0x08;
const BUFFER_INDEX = 0x10;
const BUFFER_VERTEX = 0x20;
const BUFFER_UNIFORM = 0x40;
const TEXTURE_RENDER_ATTACHMENT = 0x10;
const MAX_INSTANCES = 65_536;
const MAX_OVERLAY_VERTICES = 262_144;
export class WebGpuRetainedRenderer {
    #canvas;
    #device;
    #context;
    #format;
    #scenePipeline;
    #overlayPipeline;
    #uniform;
    #uniformBindGroup;
    #meshes = new Map();
    #materials = new Map();
    #overlayBuffer;
    #overlayCapacity = 6;
    #depth = null;
    #width = 0;
    #height = 0;
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
        return new WebGpuRetainedRenderer(canvas, device, context, gpu.getPreferredCanvasFormat());
    }
    constructor(canvas, device, context, format) {
        this.#canvas = canvas;
        this.#device = device;
        this.#context = context;
        this.#format = format;
        this.#scenePipeline = createScenePipeline(device, format);
        this.#overlayPipeline = createOverlayPipeline(device, format);
        this.#uniform = device.createBuffer({
            label: "Voplay retained 3D scene uniform",
            size: 128,
            usage: BUFFER_UNIFORM | BUFFER_COPY_DST,
        });
        this.#uniformBindGroup = device.createBindGroup({
            label: "Voplay retained 3D scene bind group",
            layout: this.#scenePipeline.getBindGroupLayout(0),
            entries: [{ binding: 0, resource: { buffer: this.#uniform } }],
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
        this.#device.pushErrorScope("validation");
        try {
            this.#syncAssets(assets);
            const scene = decodeScene(payload);
            this.#resize();
            const width = Math.max(1, this.#canvas.width);
            const height = Math.max(1, this.#canvas.height);
            const viewProjection = sceneViewProjection(scene.camera, width / height);
            const cameraPosition = [
                scene.camera.matrix[3] / 1000,
                scene.camera.matrix[7] / 1000,
                scene.camera.matrix[11] / 1000,
            ];
            const uniform = new Float32Array(32);
            uniform.set(viewProjection, 0);
            uniform.set([-0.36, -0.84, -0.41, 0], 16);
            uniform.set([...cameraPosition, 1], 20);
            uniform.set([...scene.fogColor, 1], 24);
            uniform.set([scene.fogStart, scene.fogEnd, 0, 0], 28);
            this.#device.queue.writeBuffer(this.#uniform, 0, uniform);
            const batches = new Map();
            for (const instance of scene.instances) {
                const mesh = this.#meshes.get(instance.mesh);
                if (mesh === undefined)
                    continue;
                const material = this.#materials.get(instance.material);
                const values = batches.get(instance.mesh) ?? [];
                pushInstance(values, instance.matrix, material?.color ?? [0.72, 0.72, 0.76, 1]);
                batches.set(instance.mesh, values);
            }
            const overlayValues = decodeOverlays(scene.overlays, width, height);
            if (overlayValues.length / 6 > MAX_OVERLAY_VERTICES) {
                throw new Error("Voplay retained 3D overlay capacity exceeded");
            }
            const overlay = new Float32Array(overlayValues);
            this.#ensureOverlayCapacity(overlay.length / 6);
            if (overlay.length > 0)
                this.#device.queue.writeBuffer(this.#overlayBuffer, 0, overlay);
            const encoder = this.#device.createCommandEncoder({
                label: "Voplay retained 3D frame",
            });
            const pass = encoder.beginRenderPass({
                label: "Voplay retained 3D render pass",
                colorAttachments: [{
                        view: this.#context.getCurrentTexture().createView(),
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
            pass.setPipeline(this.#scenePipeline);
            pass.setBindGroup(0, this.#uniformBindGroup);
            for (const [meshId, values] of batches) {
                const mesh = this.#meshes.get(meshId);
                const count = values.length / 20;
                if (count === 0)
                    continue;
                if (count > MAX_INSTANCES)
                    throw new Error("Voplay retained 3D instance capacity exceeded");
                this.#ensureInstanceCapacity(mesh, count);
                this.#device.queue.writeBuffer(mesh.instance, 0, new Float32Array(values));
                pass.setVertexBuffer(0, mesh.vertex);
                pass.setVertexBuffer(1, mesh.instance);
                pass.setIndexBuffer(mesh.index, "uint32");
                pass.drawIndexed(mesh.indexCount, count, 0, 0, 0);
            }
            if (overlay.length > 0) {
                pass.setPipeline(this.#overlayPipeline);
                pass.setVertexBuffer(0, this.#overlayBuffer);
                pass.draw(overlay.length / 6, 1, 0);
            }
            pass.end();
            this.#device.queue.submit([encoder.finish()]);
        }
        catch (error) {
            await this.#device.popErrorScope();
            throw error;
        }
        const validation = await this.#device.popErrorScope();
        if (validation !== null) {
            throw new Error(`Voplay retained WebGPU validation failed: ${validation.message ?? "unknown"}`);
        }
    }
    close() {
        if (this.#closed)
            return;
        this.#closed = true;
        for (const mesh of this.#meshes.values()) {
            mesh.vertex.destroy();
            mesh.index.destroy();
            mesh.instance.destroy();
        }
        this.#meshes.clear();
        this.#materials.clear();
        this.#overlayBuffer.destroy();
        this.#uniform.destroy();
        this.#depth?.destroy();
        this.#context.unconfigure();
        this.#device.destroy();
    }
    #syncAssets(assets) {
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
                this.#materials.set(asset.asset, {
                    revision: asset.revision,
                    color: decodeMaterial(asset.bytes, asset.asset),
                });
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
        this.#depth = this.#device.createTexture({
            label: "Voplay retained 3D depth",
            size: [width, height, 1],
            format: "depth24plus",
            usage: TEXTURE_RENDER_ATTACHMENT,
        });
    }
    #ensureInstanceCapacity(mesh, count) {
        if (mesh.instanceCapacity >= count)
            return;
        let capacity = mesh.instanceCapacity;
        while (capacity < count)
            capacity *= 2;
        mesh.instance.destroy();
        mesh.instance = this.#device.createBuffer({
            label: "Voplay retained 3D instances",
            size: capacity * 20 * 4,
            usage: BUFFER_VERTEX | BUFFER_COPY_DST,
        });
        mesh.instanceCapacity = capacity;
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
};
@group(0) @binding(0) var<uniform> scene: Scene;

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
};

@vertex fn vertex_main(input: VertexIn) -> VertexOut {
  let model = mat4x4<f32>(input.model_0, input.model_1, input.model_2, input.model_3);
  let world = model * vec4<f32>(input.position, 1.0);
  var output: VertexOut;
  output.position = scene.view_projection * world;
  output.world_position = world.xyz;
  output.world_normal = normalize((model * vec4<f32>(input.normal, 0.0)).xyz);
  output.color = input.color;
  return output;
}

@fragment fn fragment_main(input: VertexOut) -> @location(0) vec4<f32> {
  let diffuse = max(dot(input.world_normal, -scene.light_direction.xyz), 0.0);
  let hemi = input.world_normal.y * 0.16 + 0.16;
  let lit = input.color.rgb * (0.34 + diffuse * 0.72 + hemi);
  let distance_to_camera = distance(input.world_position, scene.camera_position.xyz);
  let fog = smoothstep(scene.fog_range.x, scene.fog_range.y, distance_to_camera);
  return vec4<f32>(mix(lit, scene.fog_color.rgb, fog), input.color.a);
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
            targets: [{ format }],
        },
        primitive: { topology: "triangle-list", cullMode: "back", frontFace: "ccw" },
        depthStencil: {
            format: "depth24plus",
            depthWriteEnabled: true,
            depthCompare: "less",
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
                instances.push({ mesh, material, matrix });
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
    if (bytes.byteLength < 39
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
    return [
        view.getUint16(31, true) / 65535,
        view.getUint16(33, true) / 65535,
        view.getUint16(35, true) / 65535,
        view.getUint16(37, true) / 65535,
    ];
}
function pushInstance(output, matrix, color) {
    output.push(matrix[0] / 1000, matrix[4] / 1000, matrix[8] / 1000, 0, matrix[1] / 1000, matrix[5] / 1000, matrix[9] / 1000, 0, matrix[2] / 1000, matrix[6] / 1000, matrix[10] / 1000, 0, matrix[3] / 1000, matrix[7] / 1000, matrix[11] / 1000, 1, ...color);
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
