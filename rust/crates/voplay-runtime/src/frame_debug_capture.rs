use std::collections::{BTreeMap, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::{
    buffer_lease::BufferLease,
    render_graph::{GraphNodeId, GraphResourceId},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FrameCaptureId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameCaptureRequest {
    pub id: FrameCaptureId,
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub frame_id: u64,
    pub expected_graph_signature: u64,
    pub include_attachments: bool,
    pub include_shader_diagnostics: bool,
    pub deadline_millis: u64,
    pub max_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedNode {
    pub node: GraphNodeId,
    pub label: String,
    pub queue: u8,
    pub started_nanos: u64,
    pub finished_nanos: u64,
    pub pipeline_hash: u64,
    pub draw_calls: u32,
    pub dispatch_calls: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedAttachment {
    pub resource: GraphResourceId,
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub row_bytes: u32,
    pub lease: BufferLease,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedShaderDiagnostic {
    pub pipeline_hash: u64,
    pub severity: u8,
    pub source_start: u32,
    pub source_end: u32,
    pub graph_node: GraphNodeId,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameCaptureOutcome {
    Completed,
    Cancelled,
    DeadlineExceeded,
    GraphChanged,
    EndpointRestartedBeforeCapture,
    OutcomeUnknownOnEndpointRestart,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameCaptureResult {
    pub request: FrameCaptureRequest,
    pub graph_signature: u64,
    pub outcome: FrameCaptureOutcome,
    pub nodes: Vec<CapturedNode>,
    pub attachments: Vec<CapturedAttachment>,
    pub shader_diagnostics: Vec<CapturedShaderDiagnostic>,
    pub diagnostic: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameCaptureConfig {
    pub max_pending: usize,
    pub max_results: usize,
    pub max_nodes_per_capture: usize,
    pub max_attachments_per_capture: usize,
    pub max_diagnostics_per_capture: usize,
    pub max_bytes_per_capture: usize,
}

impl Default for FrameCaptureConfig {
    fn default() -> Self {
        Self {
            max_pending: 8,
            max_results: 32,
            max_nodes_per_capture: 4096,
            max_attachments_per_capture: 128,
            max_diagnostics_per_capture: 1024,
            max_bytes_per_capture: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameCaptureError {
    Closed,
    InvalidConfig,
    InvalidRequest,
    DuplicateRequest,
    PendingCapacity,
    ResultCapacity,
    UnknownRequest,
    WrongEndpoint,
    GraphMismatch,
    CaptureCapacity,
    ByteCapacity,
    InvalidRecord,
    AlreadyCapturing,
    NotCapturing,
    ClockRegression,
}

#[derive(Clone, Debug)]
struct PendingCapture {
    request: FrameCaptureRequest,
    graph_signature: u64,
    capturing: bool,
    submitted: bool,
    bytes: usize,
    nodes: Vec<CapturedNode>,
    attachments: Vec<CapturedAttachment>,
    shader_diagnostics: Vec<CapturedShaderDiagnostic>,
}

pub struct FrameDebugCaptureQueue {
    engine: EngineId,
    endpoint_generation: Handle,
    config: FrameCaptureConfig,
    pending: BTreeMap<FrameCaptureId, PendingCapture>,
    results: VecDeque<FrameCaptureResult>,
    last_request: u64,
    last_clock_millis: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameDebugCaptureOwnerSnapshot {
    pub engine: EngineId,
    pub endpoint_generation: Handle,
    pub closed: bool,
    pub pending: usize,
    pub terminal: usize,
    pub attachment_leases: usize,
    pub owned_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameDebugCaptureShutdownReport {
    pub released_pending: usize,
    pub released_terminal: usize,
    pub released_attachment_leases: usize,
    pub released_bytes: usize,
}

impl FrameDebugCaptureQueue {
    pub fn request(&self, id: FrameCaptureId) -> Option<FrameCaptureRequest> {
        self.pending.get(&id).map(|capture| capture.request)
    }

    pub fn owner_snapshot(&self) -> FrameDebugCaptureOwnerSnapshot {
        FrameDebugCaptureOwnerSnapshot {
            engine: self.engine,
            endpoint_generation: self.endpoint_generation,
            closed: self.closed,
            pending: self.pending.len(),
            terminal: self.results.len(),
            attachment_leases: self
                .pending
                .values()
                .map(|capture| capture.attachments.len())
                .sum::<usize>()
                + self
                    .results
                    .iter()
                    .map(|result| result.attachments.len())
                    .sum::<usize>(),
            owned_bytes: self
                .pending
                .values()
                .map(|capture| capture.bytes)
                .sum::<usize>()
                + self
                    .results
                    .iter()
                    .map(|result| {
                        result
                            .nodes
                            .iter()
                            .map(|node| node.label.len() + 64)
                            .sum::<usize>()
                            + result
                                .attachments
                                .iter()
                                .map(|attachment| attachment.lease.len)
                                .sum::<usize>()
                            + result
                                .shader_diagnostics
                                .iter()
                                .map(|diagnostic| diagnostic.message.len() + 32)
                                .sum::<usize>()
                    })
                    .sum::<usize>(),
        }
    }

    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: FrameCaptureConfig,
    ) -> Result<Self, FrameCaptureError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_pending == 0
            || config.max_results == 0
            || config.max_nodes_per_capture == 0
            || config.max_attachments_per_capture == 0
            || config.max_diagnostics_per_capture == 0
            || config.max_bytes_per_capture == 0
        {
            return Err(FrameCaptureError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            pending: BTreeMap::new(),
            results: VecDeque::new(),
            last_request: 0,
            last_clock_millis: 0,
            closed: false,
        })
    }

    pub fn submit(&mut self, request: FrameCaptureRequest) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if request.id.0 == 0
            || request.id.0 <= self.last_request
            || request.engine != self.engine
            || request.endpoint_generation != self.endpoint_generation
            || request.frame_id == 0
            || request.expected_graph_signature == 0
            || request.deadline_millis == 0
            || request.max_bytes == 0
            || request.max_bytes > self.config.max_bytes_per_capture
        {
            return Err(FrameCaptureError::InvalidRequest);
        }
        if self.pending.contains_key(&request.id) {
            return Err(FrameCaptureError::DuplicateRequest);
        }
        if self.pending.len() == self.config.max_pending {
            return Err(FrameCaptureError::PendingCapacity);
        }
        self.last_request = request.id.0;
        self.pending.insert(
            request.id,
            PendingCapture {
                request,
                graph_signature: 0,
                capturing: false,
                submitted: false,
                bytes: 0,
                nodes: Vec::new(),
                attachments: Vec::new(),
                shader_diagnostics: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn begin_frame(
        &mut self,
        frame_id: u64,
        graph_signature: u64,
        now_millis: u64,
    ) -> Result<Vec<FrameCaptureId>, FrameCaptureError> {
        self.ensure_open()?;
        self.advance_clock(now_millis)?;
        let mut started = Vec::new();
        for capture in self.pending.values_mut() {
            if capture.capturing || capture.submitted || capture.request.frame_id != frame_id {
                continue;
            }
            if capture.request.expected_graph_signature != graph_signature {
                capture.graph_signature = graph_signature;
                capture.submitted = true;
                continue;
            }
            capture.graph_signature = graph_signature;
            capture.capturing = true;
            started.push(capture.request.id);
        }
        let mismatched = self
            .pending
            .iter()
            .filter(|(_, capture)| {
                capture.submitted
                    && !capture.capturing
                    && capture.request.frame_id == frame_id
                    && capture.request.expected_graph_signature != graph_signature
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in mismatched {
            self.finish(
                id,
                FrameCaptureOutcome::GraphChanged,
                "render graph changed",
            )?;
        }
        Ok(started)
    }

    pub fn record_node(
        &mut self,
        id: FrameCaptureId,
        node: CapturedNode,
    ) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if node.node.0 == 0
            || node.label.is_empty()
            || node.finished_nanos < node.started_nanos
            || node.pipeline_hash == 0
        {
            return Err(FrameCaptureError::InvalidRecord);
        }
        let max_nodes = self.config.max_nodes_per_capture;
        let capture = self.capturing_mut(id)?;
        if capture.nodes.len() == max_nodes {
            return Err(FrameCaptureError::CaptureCapacity);
        }
        let bytes = node
            .label
            .len()
            .checked_add(64)
            .ok_or(FrameCaptureError::ByteCapacity)?;
        reserve_bytes(capture, bytes)?;
        capture.nodes.push(node);
        Ok(())
    }

    pub fn record_attachment(
        &mut self,
        id: FrameCaptureId,
        attachment: CapturedAttachment,
    ) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if attachment.resource.0 == 0
            || attachment.version == 0
            || attachment.width == 0
            || attachment.height == 0
            || attachment.format == 0
            || attachment.row_bytes == 0
        {
            return Err(FrameCaptureError::InvalidRecord);
        }
        let max_attachments_per_capture = self.config.max_attachments_per_capture;
        let capture = self.capturing_mut(id)?;
        if !capture.request.include_attachments {
            return Err(FrameCaptureError::InvalidRecord);
        }
        if capture.attachments.len() == max_attachments_per_capture {
            return Err(FrameCaptureError::CaptureCapacity);
        }
        reserve_bytes(capture, attachment.lease.len)?;
        capture.attachments.push(attachment);
        Ok(())
    }

    pub fn record_shader_diagnostic(
        &mut self,
        id: FrameCaptureId,
        diagnostic: CapturedShaderDiagnostic,
    ) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if diagnostic.pipeline_hash == 0
            || diagnostic.severity == 0
            || diagnostic.source_end < diagnostic.source_start
            || diagnostic.graph_node.0 == 0
            || diagnostic.message.is_empty()
        {
            return Err(FrameCaptureError::InvalidRecord);
        }
        let max_diagnostics = self.config.max_diagnostics_per_capture;
        let capture = self.capturing_mut(id)?;
        if !capture.request.include_shader_diagnostics {
            return Err(FrameCaptureError::InvalidRecord);
        }
        if capture.shader_diagnostics.len() == max_diagnostics {
            return Err(FrameCaptureError::CaptureCapacity);
        }
        let bytes = diagnostic
            .message
            .len()
            .checked_add(32)
            .ok_or(FrameCaptureError::ByteCapacity)?;
        reserve_bytes(capture, bytes)?;
        capture.shader_diagnostics.push(diagnostic);
        Ok(())
    }

    pub fn complete(&mut self, id: FrameCaptureId) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if !self
            .pending
            .get(&id)
            .map(|capture| capture.capturing)
            .unwrap_or(false)
        {
            return Err(FrameCaptureError::NotCapturing);
        }
        self.finish(id, FrameCaptureOutcome::Completed, "")
    }

    pub fn fail(&mut self, id: FrameCaptureId, diagnostic: &str) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if diagnostic.is_empty() {
            return Err(FrameCaptureError::InvalidRecord);
        }
        self.finish(id, FrameCaptureOutcome::Failed, diagnostic)
    }

    pub fn cancel(&mut self, id: FrameCaptureId) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        self.finish(id, FrameCaptureOutcome::Cancelled, "capture cancelled")
    }

    pub fn expire(&mut self, now_millis: u64) -> Result<usize, FrameCaptureError> {
        self.ensure_open()?;
        self.advance_clock(now_millis)?;
        let expired = self
            .pending
            .iter()
            .filter(|(_, capture)| capture.request.deadline_millis <= now_millis)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in &expired {
            self.finish(
                *id,
                FrameCaptureOutcome::DeadlineExceeded,
                "capture deadline exceeded",
            )?;
        }
        Ok(expired.len())
    }

    pub fn restart_endpoint(&mut self, replacement: Handle) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if !replacement.is_valid() || replacement == self.endpoint_generation {
            return Err(FrameCaptureError::WrongEndpoint);
        }
        let pending = self.pending.keys().copied().collect::<Vec<_>>();
        for id in pending {
            let outcome = if self
                .pending
                .get(&id)
                .map(|capture| capture.capturing)
                .unwrap_or(false)
            {
                FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart
            } else {
                FrameCaptureOutcome::EndpointRestartedBeforeCapture
            };
            self.finish(id, outcome, "render endpoint restarted")?;
        }
        self.endpoint_generation = replacement;
        Ok(())
    }

    pub fn device_restarted(&mut self) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        let pending = self.pending.keys().copied().collect::<Vec<_>>();
        for id in pending {
            let outcome = if self
                .pending
                .get(&id)
                .is_some_and(|capture| capture.capturing)
            {
                FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart
            } else {
                FrameCaptureOutcome::EndpointRestartedBeforeCapture
            };
            self.finish(id, outcome, "render device restarted")?;
        }
        Ok(())
    }

    pub fn fail_all(&mut self, diagnostic: &str) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if diagnostic.is_empty() {
            return Err(FrameCaptureError::InvalidRecord);
        }
        for id in self.pending.keys().copied().collect::<Vec<_>>() {
            if let Some(capture) = self.pending.get_mut(&id) {
                capture.attachments.clear();
            }
            self.finish(id, FrameCaptureOutcome::Failed, diagnostic)?;
        }
        Ok(())
    }

    pub fn take_results(&mut self, max: usize) -> Vec<FrameCaptureResult> {
        self.results.drain(..max.min(self.results.len())).collect()
    }

    pub fn shutdown(&mut self) -> FrameDebugCaptureShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.pending.clear();
        self.results.clear();
        FrameDebugCaptureShutdownReport {
            released_pending: snapshot.pending,
            released_terminal: snapshot.terminal,
            released_attachment_leases: snapshot.attachment_leases,
            released_bytes: snapshot.owned_bytes,
        }
    }

    fn capturing_mut(
        &mut self,
        id: FrameCaptureId,
    ) -> Result<&mut PendingCapture, FrameCaptureError> {
        let capture = self
            .pending
            .get_mut(&id)
            .ok_or(FrameCaptureError::UnknownRequest)?;
        if !capture.capturing {
            return Err(FrameCaptureError::NotCapturing);
        }
        Ok(capture)
    }

    fn finish(
        &mut self,
        id: FrameCaptureId,
        outcome: FrameCaptureOutcome,
        diagnostic: &str,
    ) -> Result<(), FrameCaptureError> {
        if self.results.len() == self.config.max_results {
            return Err(FrameCaptureError::ResultCapacity);
        }
        let capture = self
            .pending
            .remove(&id)
            .ok_or(FrameCaptureError::UnknownRequest)?;
        self.results.push_back(FrameCaptureResult {
            request: capture.request,
            graph_signature: capture.graph_signature,
            outcome,
            nodes: capture.nodes,
            attachments: capture.attachments,
            shader_diagnostics: capture.shader_diagnostics,
            diagnostic: diagnostic.to_string(),
        });
        Ok(())
    }

    fn advance_clock(&mut self, now_millis: u64) -> Result<(), FrameCaptureError> {
        self.ensure_open()?;
        if now_millis < self.last_clock_millis {
            return Err(FrameCaptureError::ClockRegression);
        }
        self.last_clock_millis = now_millis;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), FrameCaptureError> {
        if self.closed {
            Err(FrameCaptureError::Closed)
        } else {
            Ok(())
        }
    }
}

fn reserve_bytes(capture: &mut PendingCapture, bytes: usize) -> Result<(), FrameCaptureError> {
    capture.bytes = capture
        .bytes
        .checked_add(bytes)
        .filter(|total| *total <= capture.request.max_bytes)
        .ok_or(FrameCaptureError::ByteCapacity)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn request(id: u64, frame_id: u64, generation: Handle) -> FrameCaptureRequest {
        FrameCaptureRequest {
            id: FrameCaptureId(id),
            engine: handle(1, 1),
            endpoint_generation: generation,
            frame_id,
            expected_graph_signature: 7,
            include_attachments: false,
            include_shader_diagnostics: false,
            deadline_millis: 100,
            max_bytes: 1024,
        }
    }

    #[test]
    fn graph_mismatch_and_endpoint_restart_produce_honest_terminal_outcomes() {
        let generation = handle(2, 1);
        let mut captures =
            FrameDebugCaptureQueue::new(handle(1, 1), generation, FrameCaptureConfig::default())
                .unwrap();
        captures.submit(request(1, 10, generation)).unwrap();
        assert!(captures.begin_frame(10, 8, 1).unwrap().is_empty());
        assert_eq!(
            captures.take_results(1)[0].outcome,
            FrameCaptureOutcome::GraphChanged
        );

        captures.submit(request(2, 11, generation)).unwrap();
        captures.submit(request(3, 12, generation)).unwrap();
        assert_eq!(
            captures.begin_frame(11, 7, 2).unwrap(),
            vec![FrameCaptureId(2)]
        );
        captures.restart_endpoint(handle(2, 2)).unwrap();
        let results = captures.take_results(8);
        assert_eq!(
            results[0].outcome,
            FrameCaptureOutcome::OutcomeUnknownOnEndpointRestart
        );
        assert_eq!(
            results[1].outcome,
            FrameCaptureOutcome::EndpointRestartedBeforeCapture
        );
    }

    #[test]
    fn capture_records_valid_nodes_and_enforces_request_byte_budget() {
        let generation = handle(2, 1);
        let config = FrameCaptureConfig::default();
        let mut captures = FrameDebugCaptureQueue::new(handle(1, 1), generation, config).unwrap();
        let mut limited = request(1, 10, generation);
        limited.max_bytes = 68;
        captures.submit(limited).unwrap();
        captures.begin_frame(10, 7, 1).unwrap();
        captures
            .record_node(
                FrameCaptureId(1),
                CapturedNode {
                    node: GraphNodeId(1),
                    label: "node".to_owned(),
                    queue: 1,
                    started_nanos: 1,
                    finished_nanos: 2,
                    pipeline_hash: 3,
                    draw_calls: 1,
                    dispatch_calls: 0,
                },
            )
            .unwrap();
        assert_eq!(
            captures.record_node(
                FrameCaptureId(1),
                CapturedNode {
                    node: GraphNodeId(2),
                    label: "x".to_owned(),
                    queue: 1,
                    started_nanos: 1,
                    finished_nanos: 2,
                    pipeline_hash: 4,
                    draw_calls: 1,
                    dispatch_calls: 0,
                },
            ),
            Err(FrameCaptureError::ByteCapacity)
        );
        captures.complete(FrameCaptureId(1)).unwrap();
        let result = captures.take_results(1).pop().unwrap();
        assert_eq!(result.outcome, FrameCaptureOutcome::Completed);
        assert_eq!(result.nodes.len(), 1);
    }
}
