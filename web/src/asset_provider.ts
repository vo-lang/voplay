import {
  MessageKind,
  decodeFrameworkPacket,
  encodeFrameworkPacket,
  type FrameworkPacket,
  type GenerationalHandle,
} from "../../protocol/generated/voplay_protocol.js";

interface StudioFrameworkLane {
  readonly binding: {
    readonly channelEpoch: number;
    readonly caller: {
      readonly endpointIndex: number;
      readonly endpointGeneration: number;
    };
  };
  poll(): Promise<Uint8Array | null>;
  submit(payload: Uint8Array, requestId?: bigint): Promise<void>;
  close(): void;
}

interface StudioProviderHost {
  readonly framework: Readonly<{
    name: string;
    providerRoles: readonly string[];
  }>;
  log(message: string): void;
  reportError(message: string): void;
  getCapability(name: "framework_lane"): {
    open(role?: string): Promise<StudioFrameworkLane>;
  } | null;
  getCapability(name: "asset_buffer"): BrowserAssetBufferCapability | null;
}

interface BrowserAssetBufferCapability {
  bind(asset: GenerationalHandle, artifactId: Uint8Array): Promise<void>;
  read(asset: GenerationalHandle): Promise<ArrayBuffer>;
  release(asset: GenerationalHandle): void;
}

interface AssetRegistration {
  readonly assetId: Uint8Array;
  readonly assetType: bigint;
  readonly sourceRevision: bigint;
  readonly artifactId: Uint8Array;
  readonly dependencies: Uint8Array[];
}

interface AssetRecord extends AssetRegistration {
  readonly assetRef: GenerationalHandle;
}

interface ScopeRecord {
  readonly handle: GenerationalHandle;
  readonly kind: number;
  open: boolean;
}

interface TicketRecord {
  readonly handle: GenerationalHandle;
  readonly scope: GenerationalHandle;
  readonly assetRef: GenerationalHandle;
  readonly deadline: bigint;
  state: "pending" | "dispatched" | "terminal";
}

type AssetProviderState = "created" | "running" | "suspended" | "closed";

const MAX_ASSETS = 65_536;
const MAX_DEPENDENCIES = 65_536;
const MAX_SCOPES = 4096;
const MAX_TICKETS = 65_536;
const ASSET_REQUEST_BYTES = 24;
const ASSET_WORK_BYTES = 64;

export class VoplayBrowserAssetProvider {
  #host: StudioProviderHost | null = null;
  #lane: StudioFrameworkLane | null = null;
  #assetBuffers: BrowserAssetBufferCapability | null = null;
  #polling = false;
  #state: AssetProviderState = "created";
  #engine: GenerationalHandle | null = null;
  #endpointGeneration: GenerationalHandle = { index: 0, generation: 1 };
  #lastSequence = 0n;
  #revision = 1n;
  #assetsById = new Map<string, AssetRecord>();
  #assetsByRef = new Map<string, AssetRecord>();
  #scopes = new Map<string, ScopeRecord>();
  #tickets = new Map<string, TicketRecord>();
  #nextAsset = 0;
  #nextScope = 0;
  #nextTicket = 0;
  #freeScopes: number[] = [];
  #scopeGenerations: number[] = [];
  #freeTickets: number[] = [];
  #ticketGenerations: number[] = [];

  async init(host: StudioProviderHost): Promise<void> {
    if (this.#host !== null) throw new Error("Voplay asset provider already initialized");
    if (this.#state === "closed") {
      this.#state = "created";
      this.#lastSequence = 0n;
      this.#revision = 1n;
      this.#nextAsset = 0;
      this.#nextScope = 0;
      this.#nextTicket = 0;
      this.#freeScopes = [];
      this.#scopeGenerations = [];
      this.#freeTickets = [];
      this.#ticketGenerations = [];
    }
    if (!host.framework.providerRoles.includes("game-asset")) {
      throw new Error("Voplay asset provider requires the game-asset role");
    }
    const lanes = host.getCapability("framework_lane");
    if (lanes === null) throw new Error("Voplay asset provider requires framework_lane");
    const assetBuffers = host.getCapability("asset_buffer");
    if (assetBuffers === null) throw new Error("Voplay asset provider requires asset_buffer");
    const lane = await lanes.open("asset");
    this.#endpointGeneration = {
      index: lane.binding.caller.endpointIndex,
      generation: lane.binding.caller.endpointGeneration,
    };
    if (
      this.#endpointGeneration.index === 0xffffffff
      || this.#endpointGeneration.generation === 0
    ) {
      throw new Error("Voplay asset lane has an invalid endpoint generation");
    }
    this.#host = host;
    this.#lane = lane;
    this.#assetBuffers = assetBuffers;
    this.#polling = true;
    setTimeout(() => {
      if (this.#polling && this.#host === host && this.#lane === lane) {
        void this.#poll(host, lane);
      }
    }, 0);
    host.log(`Voplay browser asset provider ready for ${host.framework.name}`);
  }

  stop(): void {
    this.#polling = false;
    this.#lane?.close();
    this.#clear();
    this.#lane = null;
    this.#assetBuffers = null;
    this.#host = null;
    this.#state = "closed";
  }

  quiesceForCapture(): { stopped: number; assets: number; tickets: number } {
    return { stopped: 1, assets: this.#assetsById.size, tickets: this.#tickets.size };
  }

  ownerSnapshot(): Readonly<{
    state: AssetProviderState;
    revision: bigint;
    endpointGeneration: GenerationalHandle;
    assets: number;
    openScopes: number;
    pendingTickets: number;
    dispatchedTickets: number;
    terminalTickets: number;
  }> {
    let openScopes = 0;
    for (const scope of this.#scopes.values()) {
      if (scope.open) openScopes += 1;
    }
    let pendingTickets = 0;
    let dispatchedTickets = 0;
    let terminalTickets = 0;
    for (const ticket of this.#tickets.values()) {
      if (ticket.state === "pending") pendingTickets += 1;
      else if (ticket.state === "dispatched") dispatchedTickets += 1;
      else terminalTickets += 1;
    }
    return {
      state: this.#state,
      revision: this.#revision,
      endpointGeneration: { ...this.#endpointGeneration },
      assets: this.#assetsById.size,
      openScopes,
      pendingTickets,
      dispatchedTickets,
      terminalTickets,
    };
  }

  async #poll(host: StudioProviderHost, lane: StudioFrameworkLane): Promise<void> {
    while (this.#polling && this.#host === host && this.#lane === lane) {
      try {
        const bytes = await lane.poll();
        if (!this.#polling || this.#host !== host || this.#lane !== lane) return;
        if (bytes === null) {
          await delay(8);
          continue;
        }
        await this.#dispatch(decodeFrameworkPacket(bytes));
      } catch (error) {
        if (!this.#polling || this.#host !== host || this.#lane !== lane) return;
        this.#polling = false;
        host.reportError(`Voplay asset provider failed: ${errorMessage(error)}`);
      }
    }
  }

  async #dispatch(packet: FrameworkPacket): Promise<void> {
    this.#validateEnvelope(packet);
    const { header, payload } = packet;
    switch (header.kind) {
      case MessageKind.EngineStart:
        if (this.#state !== "created" || payload.byteLength !== 0) {
          throw new Error("invalid Voplay asset EngineStart");
        }
        this.#engine = header.engine;
        this.#state = "running";
        await this.#reply(packet, MessageKind.EngineReady, new Uint8Array());
        return;
      case MessageKind.AssetRequest:
        this.#requireState(header.engine, "running");
        await this.#request(packet);
        return;
      case MessageKind.AssetControl:
        this.#requireState(header.engine, "running", "suspended");
        await this.#control(packet);
        return;
      case MessageKind.WorkerWake:
        this.#requireState(header.engine, "running");
        if (payload.byteLength !== 0) throw new Error("asset wake payload must be empty");
        await this.#flushWork(packet);
        return;
      case MessageKind.EngineSuspend:
        this.#requireState(header.engine, "running");
        if (payload.byteLength !== 0) throw new Error("asset suspend payload must be empty");
        this.#state = "suspended";
        return;
      case MessageKind.EngineResume:
        this.#requireState(header.engine, "suspended");
        if (payload.byteLength !== 0) throw new Error("asset resume payload must be empty");
        this.#state = "running";
        await this.#flushWork(packet);
        return;
      case MessageKind.EngineClose:
        this.#requireState(header.engine, "created", "running", "suspended");
        if (payload.byteLength !== 0) throw new Error("asset close payload must be empty");
        for (const ticket of this.#tickets.values()) {
          if (ticket.state !== "terminal") await this.#finishTicket(packet, ticket, 4);
        }
        this.#state = "closed";
        await this.#reply(packet, MessageKind.EngineClosed, new Uint8Array());
        this.#clear();
        return;
      default:
        throw new Error(`unsupported Voplay asset packet ${header.kind}`);
    }
  }

  async #request(packet: FrameworkPacket): Promise<void> {
    const payload = packet.payload;
    if (payload.byteLength !== ASSET_REQUEST_BYTES || packet.header.sequence === 0n) {
      throw new Error("invalid Voplay AssetRequest");
    }
    const scope = readHandle(payload, 0);
    const assetRef = readHandle(payload, 8);
    const deadline = readU64(payload, 16);
    const scopeRecord = this.#scopes.get(handleKey(scope));
    if (!scopeRecord?.open || !this.#assetsByRef.has(handleKey(assetRef))) {
      throw new Error("Voplay AssetRequest references an unknown scope or asset");
    }
    if (this.#tickets.size >= MAX_TICKETS) throw new Error("Voplay asset ticket capacity exceeded");
    const ticket: TicketRecord = {
      handle: this.#allocateTicketHandle(),
      scope,
      assetRef,
      deadline,
      state: "pending",
    };
    this.#tickets.set(handleKey(ticket.handle), ticket);
    this.#touch();
    await this.#reply(packet, MessageKind.AssetCompletion, concatBytes(
      Uint8Array.of(2),
      encodeHandle(ticket.handle),
      encodeHandle(assetRef),
    ));
    await this.#flushWork(packet);
  }

  async #control(packet: FrameworkPacket): Promise<void> {
    const tag = packet.payload[0];
    const body = packet.payload.subarray(1);
    switch (tag) {
      case 1:
        await this.#register(packet, decodeRegistrations(body, true), false);
        return;
      case 2:
        await this.#register(packet, decodeRegistrations(body, false), true);
        return;
      case 3: {
        if (body.byteLength !== 1) {
          throw new Error("invalid Voplay asset scope kind");
        }
        const kind = body[0]!;
        if (kind < 1 || kind > 4) {
          throw new Error("invalid Voplay asset scope kind");
        }
        if (this.#scopes.size >= MAX_SCOPES) throw new Error("Voplay asset scope capacity exceeded");
        const scope: ScopeRecord = {
          handle: this.#allocateScopeHandle(),
          kind,
          open: true,
        };
        this.#scopes.set(handleKey(scope.handle), scope);
        this.#touch();
        await this.#reply(packet, MessageKind.AssetCompletion, concatBytes(
          Uint8Array.of(1),
          encodeHandle(scope.handle),
          Uint8Array.of(scope.kind),
        ));
        return;
      }
      case 4:
        await this.#terminalControl(packet, body, 2, false);
        return;
      case 5:
        await this.#closeScope(packet, body);
        return;
      case 6:
        this.#releaseTicket(body);
        await this.#controlAck(packet, 6);
        return;
      case 7:
        this.#releaseScope(body);
        await this.#controlAck(packet, 7);
        return;
      case 8:
        await this.#expire(packet, body);
        await this.#controlAck(packet, 8);
        return;
      case 9:
        await this.#terminalizeWork(packet, body, 1);
        return;
      case 10:
        if (body.byteLength !== 0) throw new Error("invalid Voplay asset restart");
        if (this.#endpointGeneration.generation === 0xffffffff) {
          throw new Error("Voplay asset endpoint generation exhausted");
        }
        this.#endpointGeneration = {
          index: this.#endpointGeneration.index,
          generation: this.#endpointGeneration.generation + 1,
        };
        for (const ticket of this.#tickets.values()) {
          if (ticket.state === "dispatched") ticket.state = "pending";
        }
        this.#touch();
        await this.#controlAck(packet, 10);
        await this.#flushWork(packet);
        return;
      case 11:
        await this.#terminalizeWork(packet, body, 5);
        return;
      default:
        throw new Error("unsupported Voplay AssetControl command");
    }
  }

  async #register(
    packet: FrameworkPacket,
    registrations: AssetRegistration[],
    hotReload: boolean,
  ): Promise<void> {
    if (hotReload && registrations.length !== 1) throw new Error("invalid asset hot reload batch");
    if (!hotReload && this.#assetsById.size + registrations.length > MAX_ASSETS) {
      throw new Error("Voplay asset capacity exceeded");
    }
    const nextGraph = new Map<string, string[]>();
    for (const [assetId, asset] of this.#assetsById) {
      nextGraph.set(assetId, asset.dependencies.map(bytesKey));
    }
    for (const registration of registrations) {
      const key = bytesKey(registration.assetId);
      const current = this.#assetsById.get(key);
      if (hotReload) {
        if (!current || registration.sourceRevision <= current.sourceRevision) {
          throw new Error("stale or unknown Voplay asset hot reload");
        }
      } else if (current) {
        throw new Error("duplicate Voplay asset registration");
      }
      nextGraph.set(key, registration.dependencies.map(bytesKey));
    }
    validateDependencyGraph(nextGraph);
    const assetBuffers = this.#assetBuffers;
    if (assetBuffers === null) throw new Error("Voplay asset buffer capability is unavailable");
    const refs: GenerationalHandle[] = [];
    const prepared: AssetRecord[] = [];
    for (const registration of registrations) {
      const key = bytesKey(registration.assetId);
      const current = this.#assetsById.get(key);
      if (hotReload) {
        if (!current) throw new Error("Voplay asset hot reload preflight diverged");
        prepared.push({ ...registration, assetRef: current.assetRef });
        refs.push(current.assetRef);
      } else {
        const assetRef = allocateHandle(this.#nextAsset++);
        prepared.push({ ...registration, assetRef });
        refs.push(assetRef);
      }
    }
    const attempted: AssetRecord[] = [];
    try {
      for (const record of prepared) {
        attempted.push(record);
        await assetBuffers.bind(record.assetRef, record.artifactId);
      }
    } catch (error) {
      for (const record of attempted.reverse()) {
        const current = this.#assetsById.get(bytesKey(record.assetId));
        if (current === undefined) {
          assetBuffers.release(record.assetRef);
          continue;
        }
        try {
          await assetBuffers.bind(current.assetRef, current.artifactId);
        } catch (rollbackError) {
          this.#host?.reportError(
            `Voplay asset binding rollback failed: ${errorMessage(rollbackError)}`,
          );
        }
      }
      throw error;
    }
    for (const record of prepared) {
      this.#assetsById.set(bytesKey(record.assetId), record);
      this.#assetsByRef.set(handleKey(record.assetRef), record);
      if (hotReload) {
        for (const ticket of this.#tickets.values()) {
          if (sameHandle(ticket.assetRef, record.assetRef) && ticket.state === "dispatched") {
            ticket.state = "pending";
          }
        }
      }
    }
    this.#touch();
    const count = new Uint8Array(4);
    new DataView(count.buffer).setUint32(0, refs.length, true);
    await this.#reply(packet, MessageKind.AssetCompletion, concatBytes(
      Uint8Array.of(5),
      count,
      ...refs.map(encodeHandle),
    ));
    if (hotReload) await this.#flushWork(packet);
  }

  async #terminalControl(
    packet: FrameworkPacket,
    body: Uint8Array,
    outcome: number,
    allowMissing: boolean,
  ): Promise<void> {
    if (body.byteLength !== 8) throw new Error("invalid Voplay asset ticket handle");
    const ticket = this.#tickets.get(handleKey(readHandle(body, 0)));
    if (!ticket) {
      if (allowMissing) return;
      throw new Error("unknown Voplay asset ticket");
    }
    if (ticket.state !== "terminal") await this.#finishTicket(packet, ticket, outcome);
  }

  async #closeScope(packet: FrameworkPacket, body: Uint8Array): Promise<void> {
    if (body.byteLength !== 8) throw new Error("invalid Voplay asset scope handle");
    const scope = this.#scopes.get(handleKey(readHandle(body, 0)));
    if (!scope || !scope.open) throw new Error("unknown or closed Voplay asset scope");
    scope.open = false;
    this.#touch();
    for (const ticket of this.#tickets.values()) {
      if (sameHandle(ticket.scope, scope.handle) && ticket.state !== "terminal") {
        await this.#finishTicket(packet, ticket, 2);
      }
    }
  }

  #releaseTicket(body: Uint8Array): void {
    if (body.byteLength !== 8) throw new Error("invalid Voplay asset ticket release");
    const key = handleKey(readHandle(body, 0));
    const ticket = this.#tickets.get(key);
    if (!ticket || ticket.state !== "terminal") {
      throw new Error("unknown Voplay asset ticket release");
    }
    this.#tickets.delete(key);
    this.#retireTicketHandle(ticket.handle);
    this.#touch();
  }

  #releaseScope(body: Uint8Array): void {
    if (body.byteLength !== 8) throw new Error("invalid Voplay asset scope release");
    const key = handleKey(readHandle(body, 0));
    const scope = this.#scopes.get(key);
    if (!scope || scope.open) throw new Error("Voplay asset scope must be closed before release");
    for (const ticket of this.#tickets.values()) {
      if (sameHandle(ticket.scope, scope.handle)) {
        throw new Error("Voplay asset scope still owns tickets");
      }
    }
    this.#scopes.delete(key);
    this.#retireScopeHandle(scope.handle);
    this.#touch();
  }

  async #expire(packet: FrameworkPacket, body: Uint8Array): Promise<void> {
    if (body.byteLength !== 8) throw new Error("invalid Voplay asset expiry");
    const now = readU64(body, 0);
    for (const ticket of this.#tickets.values()) {
      if (ticket.state !== "terminal" && ticket.deadline <= now) {
        await this.#finishTicket(packet, ticket, 3);
      }
    }
  }

  async #terminalizeWork(
    packet: FrameworkPacket,
    body: Uint8Array,
    outcome: 1 | 5,
  ): Promise<void> {
    if (body.byteLength !== ASSET_WORK_BYTES) throw new Error("invalid Voplay asset work result");
    const ticket = this.#tickets.get(handleKey(readHandle(body, 0)));
    const assetRef = readHandle(body, 8);
    const resultEndpoint = readHandle(body, 56);
    if (ticket === undefined || ticket.state === "terminal") return;
    if (
      !sameHandle(ticket.assetRef, assetRef)
      || resultEndpoint.index !== this.#endpointGeneration.index
    ) {
      throw new Error("stale Voplay asset work result");
    }
    if (resultEndpoint.generation < this.#endpointGeneration.generation) return;
    if (resultEndpoint.generation > this.#endpointGeneration.generation) {
      throw new Error("future Voplay asset work endpoint generation");
    }
    const asset = this.#assetsByRef.get(handleKey(assetRef));
    if (!asset || bytesKey(body.subarray(16, 32)) !== bytesKey(asset.assetId)) {
      throw new Error("Voplay asset work result identity mismatch");
    }
    const resultRevision = readU64(body, 32);
    if (resultRevision < asset.sourceRevision) return;
    if (
      resultRevision > asset.sourceRevision
      || bytesKey(body.subarray(40, 56)) !== bytesKey(asset.artifactId)
    ) {
      throw new Error("Voplay asset work result artifact mismatch");
    }
    if (ticket.state !== "dispatched") return;
    for (const candidate of this.#tickets.values()) {
      if (sameHandle(candidate.assetRef, assetRef) && candidate.state !== "terminal") {
        await this.#finishTicket(packet, candidate, outcome);
      }
    }
  }

  async #flushWork(packet: FrameworkPacket): Promise<void> {
    if (this.#state !== "running") return;
    for (const ticket of this.#tickets.values()) {
      if (ticket.state !== "pending") continue;
      const asset = this.#assetsByRef.get(handleKey(ticket.assetRef));
      if (!asset) throw new Error("Voplay asset ticket lost its asset");
      ticket.state = "dispatched";
      this.#touch();
      const sourceRevision = new Uint8Array(8);
      new DataView(sourceRevision.buffer).setBigUint64(0, asset.sourceRevision, true);
      await this.#reply(packet, MessageKind.AssetCompletion, concatBytes(
        Uint8Array.of(3),
        encodeHandle(ticket.handle),
        encodeHandle(asset.assetRef),
        asset.assetId,
        sourceRevision,
        asset.artifactId,
        encodeHandle(this.#endpointGeneration),
      ));
    }
  }

  async #finishTicket(packet: FrameworkPacket, ticket: TicketRecord, outcome: number): Promise<void> {
    ticket.state = "terminal";
    this.#touch();
    await this.#reply(packet, MessageKind.AssetCompletion, concatBytes(
      Uint8Array.of(4),
      encodeHandle(ticket.handle),
      encodeHandle(ticket.assetRef),
      Uint8Array.of(outcome),
    ));
  }

  async #controlAck(packet: FrameworkPacket, command: number): Promise<void> {
    await this.#reply(packet, MessageKind.AssetCompletion, Uint8Array.of(6, command));
  }

  #validateEnvelope(packet: FrameworkPacket): void {
    const lane = this.#lane;
    if (lane === null) throw new Error("Voplay asset lane is closed");
    if (packet.header.channelEpoch !== BigInt(lane.binding.channelEpoch)) {
      throw new Error("Voplay asset packet channel epoch mismatch");
    }
    const lifecyclePacket =
      packet.header.kind === MessageKind.EngineStart
      || packet.header.kind === MessageKind.EngineSuspend
      || packet.header.kind === MessageKind.EngineResume
      || packet.header.kind === MessageKind.EngineClose
      || packet.header.kind === MessageKind.WorkerWake;
    if (!lifecyclePacket && packet.header.sequence <= this.#lastSequence) {
      throw new Error("Voplay asset packet sequence regression");
    }
    if (!lifecyclePacket) {
      this.#lastSequence = packet.header.sequence;
    }
  }

  #requireState(engine: GenerationalHandle, ...states: AssetProviderState[]): void {
    if (
      !states.includes(this.#state)
      || this.#engine === null
      || !sameHandle(this.#engine, engine)
    ) {
      throw new Error("Voplay asset provider state or engine mismatch");
    }
  }

  async #reply(packet: FrameworkPacket, kind: MessageKind, payload: Uint8Array): Promise<void> {
    const lane = this.#lane;
    if (lane === null) throw new Error("Voplay asset lane is closed");
    const source = packet.header;
    await lane.submit(encodeFrameworkPacket({
      kind,
      engine: source.engine,
      channelEpoch: source.channelEpoch,
      commitId: source.commitId,
      baseRevision: 0n,
      newRevision: this.#revision,
      requiredControlRevision: 0n,
      sourceSimulationRevision: source.sourceSimulationRevision,
      sequence: source.sequence,
    }, payload), source.sequence);
  }

  #touch(): void {
    if (this.#revision >= 0xffff_ffff_ffff_ffffn) {
      throw new Error("Voplay asset revision exhausted");
    }
    this.#revision += 1n;
  }

  #allocateScopeHandle(): GenerationalHandle {
    const index = this.#freeScopes.pop();
    if (index !== undefined) {
      return { index, generation: this.#scopeGenerations[index]! };
    }
    const handle = allocateHandle(this.#nextScope++);
    this.#scopeGenerations[handle.index] = handle.generation;
    return handle;
  }

  #retireScopeHandle(handle: GenerationalHandle): void {
    if (handle.generation === 0xffff_ffff) return;
    this.#scopeGenerations[handle.index] = handle.generation + 1;
    this.#freeScopes.push(handle.index);
  }

  #allocateTicketHandle(): GenerationalHandle {
    const index = this.#freeTickets.pop();
    if (index !== undefined) {
      return { index, generation: this.#ticketGenerations[index]! };
    }
    const handle = allocateHandle(this.#nextTicket++);
    this.#ticketGenerations[handle.index] = handle.generation;
    return handle;
  }

  #retireTicketHandle(handle: GenerationalHandle): void {
    if (handle.generation === 0xffff_ffff) return;
    this.#ticketGenerations[handle.index] = handle.generation + 1;
    this.#freeTickets.push(handle.index);
  }

  #clear(): void {
    const assetBuffers = this.#assetBuffers;
    if (assetBuffers !== null) {
      for (const asset of this.#assetsByRef.values()) {
        assetBuffers.release(asset.assetRef);
      }
    }
    this.#engine = null;
    this.#assetsById.clear();
    this.#assetsByRef.clear();
    this.#scopes.clear();
    this.#tickets.clear();
  }
}

function decodeRegistrations(bytes: Uint8Array, counted: boolean): AssetRegistration[] {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const count = counted ? requireU32(view, 0) : 1;
  if (count === 0 || count > MAX_ASSETS) throw new Error("invalid Voplay asset registration count");
  let offset = counted ? 4 : 0;
  let dependencyCount = 0;
  const registrations: AssetRegistration[] = [];
  const identities = new Set<string>();
  for (let index = 0; index < count; index += 1) {
    if (bytes.byteLength - offset < 52) throw new Error("truncated Voplay asset registration");
    const assetId = bytes.slice(offset, offset + 16);
    const key = bytesKey(assetId);
    if (isZero(assetId) || identities.has(key)) throw new Error("invalid or duplicate Voplay AssetId");
    identities.add(key);
    const assetType = readU64(bytes, offset + 16);
    const sourceRevision = readU64(bytes, offset + 24);
    const artifactId = bytes.slice(offset + 32, offset + 48);
    const dependencies = view.getUint32(offset + 48, true);
    dependencyCount += dependencies;
    if (
      assetType === 0n
      || sourceRevision === 0n
      || isZero(artifactId)
      || dependencyCount > MAX_DEPENDENCIES
    ) {
      throw new Error("invalid Voplay asset registration identity");
    }
    offset += 52;
    const dependencyBytes = dependencies * 16;
    if (dependencyBytes > bytes.byteLength - offset) {
      throw new Error("truncated Voplay asset dependencies");
    }
    const dependencyIds: Uint8Array[] = [];
    for (let dependency = 0; dependency < dependencies; dependency += 1) {
      const id = bytes.slice(offset, offset + 16);
      if (isZero(id)) throw new Error("invalid Voplay dependency AssetId");
      dependencyIds.push(id);
      offset += 16;
    }
    registrations.push({ assetId, assetType, sourceRevision, artifactId, dependencies: dependencyIds });
  }
  if (offset !== bytes.byteLength) throw new Error("trailing Voplay asset registration bytes");
  return registrations;
}

function allocateHandle(index: number): GenerationalHandle {
  if (!Number.isInteger(index) || index < 0 || index >= 0xffffffff) {
    throw new Error("Voplay browser handle space exhausted");
  }
  return { index, generation: 1 };
}

function readHandle(bytes: Uint8Array, offset: number): GenerationalHandle {
  if (bytes.byteLength - offset < 8) throw new Error("truncated Voplay handle");
  const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 8);
  const handle = { index: view.getUint32(0, true), generation: view.getUint32(4, true) };
  if (handle.index === 0xffffffff || handle.generation === 0) throw new Error("invalid Voplay handle");
  return handle;
}

function encodeHandle(handle: GenerationalHandle): Uint8Array {
  const bytes = new Uint8Array(8);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, handle.index, true);
  view.setUint32(4, handle.generation, true);
  return bytes;
}

function readU64(bytes: Uint8Array, offset: number): bigint {
  if (bytes.byteLength - offset < 8) throw new Error("truncated Voplay u64");
  return new DataView(bytes.buffer, bytes.byteOffset + offset, 8).getBigUint64(0, true);
}

function requireU32(view: DataView, offset: number): number {
  if (view.byteLength - offset < 4) throw new Error("truncated Voplay u32");
  return view.getUint32(offset, true);
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const length = parts.reduce((total, part) => total + part.byteLength, 0);
  const output = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.byteLength;
  }
  return output;
}

function validateDependencyGraph(graph: ReadonlyMap<string, readonly string[]>): void {
  for (const [asset, dependencies] of graph) {
    const unique = new Set(dependencies);
    if (unique.size !== dependencies.length || unique.has(asset)) {
      throw new Error("Voplay asset dependency cycle");
    }
    for (const dependency of dependencies) {
      if (!graph.has(dependency)) throw new Error("unknown Voplay asset dependency");
    }
  }
  const active = new Set<string>();
  const complete = new Set<string>();
  const visit = (asset: string): void => {
    if (complete.has(asset)) return;
    if (active.has(asset)) throw new Error("Voplay asset dependency cycle");
    active.add(asset);
    for (const dependency of graph.get(asset) ?? []) visit(dependency);
    active.delete(asset);
    complete.add(asset);
  };
  for (const asset of graph.keys()) visit(asset);
}

function bytesKey(bytes: Uint8Array): string {
  let key = "";
  for (const byte of bytes) key += byte.toString(16).padStart(2, "0");
  return key;
}

function handleKey(handle: GenerationalHandle): string {
  return `${handle.index}:${handle.generation}`;
}

function sameHandle(left: GenerationalHandle, right: GenerationalHandle): boolean {
  return left.index === right.index && left.generation === right.generation;
}

function isZero(bytes: Uint8Array): boolean {
  return bytes.every((byte) => byte === 0);
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default new VoplayBrowserAssetProvider();
