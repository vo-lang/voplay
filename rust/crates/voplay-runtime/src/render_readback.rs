use std::collections::{BTreeMap, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::buffer_lease::BufferLease;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReadbackRequestId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReadbackTarget {
    pub engine: EngineId,
    pub target: Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackFormat {
    Rgba8,
    Bgra8,
    Rgba16Float,
    Depth32Float,
}

impl ReadbackFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 | Self::Depth32Float => 4,
            Self::Rgba16Float => 8,
        }
    }
}

pub fn convert_rgba8_readback(
    source: &[u8],
    source_row_bytes: u32,
    width: u32,
    height: u32,
    format: ReadbackFormat,
) -> Option<(u32, Vec<u8>)> {
    if width == 0 || height == 0 || format == ReadbackFormat::Depth32Float {
        return None;
    }
    let source_row_bytes = usize::try_from(source_row_bytes).ok()?;
    let source_pixels_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    let height = usize::try_from(height).ok()?;
    let source_bytes = source_row_bytes.checked_mul(height)?;
    if source_row_bytes < source_pixels_bytes || source.len() != source_bytes {
        return None;
    }
    let output_pixels_bytes = usize::try_from(width)
        .ok()?
        .checked_mul(format.bytes_per_pixel())?;
    let output_row_bytes = align_up(output_pixels_bytes, 256)?;
    let mut output = vec![0; output_row_bytes.checked_mul(height)?];
    for row in 0..height {
        let source_row =
            &source[row * source_row_bytes..row * source_row_bytes + source_pixels_bytes];
        let output_row = &mut output[row * output_row_bytes..(row + 1) * output_row_bytes];
        match format {
            ReadbackFormat::Rgba8 => {
                output_row[..source_pixels_bytes].copy_from_slice(source_row);
            }
            ReadbackFormat::Bgra8 => {
                for (source_pixel, output_pixel) in source_row
                    .chunks_exact(4)
                    .zip(output_row.chunks_exact_mut(4))
                {
                    output_pixel.copy_from_slice(&[
                        source_pixel[2],
                        source_pixel[1],
                        source_pixel[0],
                        source_pixel[3],
                    ]);
                }
            }
            ReadbackFormat::Rgba16Float => {
                for (source_pixel, output_pixel) in source_row
                    .chunks_exact(4)
                    .zip(output_row.chunks_exact_mut(8))
                {
                    for (channel, bytes) in
                        source_pixel.iter().zip(output_pixel.chunks_exact_mut(2))
                    {
                        bytes.copy_from_slice(
                            &f32_to_f16_bits(f32::from(*channel) / 255.0).to_le_bytes(),
                        );
                    }
                }
            }
            ReadbackFormat::Depth32Float => return None,
        }
    }
    Some((u32::try_from(output_row_bytes).ok()?, output))
}

pub fn convert_rgba16f_readback(
    source: &[u8],
    source_row_bytes: u32,
    width: u32,
    height: u32,
    format: ReadbackFormat,
) -> Option<(u32, Vec<u8>)> {
    if width == 0 || height == 0 || format == ReadbackFormat::Depth32Float {
        return None;
    }
    let source_row_bytes = usize::try_from(source_row_bytes).ok()?;
    let source_pixels_bytes = usize::try_from(width).ok()?.checked_mul(8)?;
    let height = usize::try_from(height).ok()?;
    if source_row_bytes < source_pixels_bytes
        || source.len() != source_row_bytes.checked_mul(height)?
    {
        return None;
    }
    let output_pixels_bytes = usize::try_from(width)
        .ok()?
        .checked_mul(format.bytes_per_pixel())?;
    let output_row_bytes = align_up(output_pixels_bytes, 256)?;
    let mut output = vec![0; output_row_bytes.checked_mul(height)?];
    for row in 0..height {
        let source_row =
            &source[row * source_row_bytes..row * source_row_bytes + source_pixels_bytes];
        let output_row = &mut output[row * output_row_bytes..(row + 1) * output_row_bytes];
        match format {
            ReadbackFormat::Rgba16Float => {
                output_row[..source_pixels_bytes].copy_from_slice(source_row);
            }
            ReadbackFormat::Rgba8 | ReadbackFormat::Bgra8 => {
                for (source_pixel, output_pixel) in source_row
                    .chunks_exact(8)
                    .zip(output_row.chunks_exact_mut(4))
                {
                    let mut channels = [0_u8; 4];
                    for (index, bytes) in source_pixel.chunks_exact(2).enumerate() {
                        let value = f16_bits_to_f32(u16::from_le_bytes([bytes[0], bytes[1]]));
                        channels[index] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    if format == ReadbackFormat::Bgra8 {
                        channels.swap(0, 2);
                    }
                    output_pixel.copy_from_slice(&channels);
                }
            }
            ReadbackFormat::Depth32Float => return None,
        }
    }
    Some((u32::try_from(output_row_bytes).ok()?, output))
}

fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        if exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x80_0000;
        let shift = u32::try_from(14 - exponent).unwrap_or(24);
        let mut rounded = mantissa >> shift;
        let remainder = mantissa & ((1_u32 << shift) - 1);
        let halfway = 1_u32 << (shift - 1);
        if remainder > halfway || remainder == halfway && rounded & 1 != 0 {
            rounded += 1;
        }
        return sign | rounded as u16;
    }
    if exponent >= 31 {
        return sign | 0x7c00;
    }
    let mut rounded = mantissa >> 13;
    let remainder = mantissa & 0x1fff;
    if remainder > 0x1000 || remainder == 0x1000 && rounded & 1 != 0 {
        rounded += 1;
        if rounded == 0x400 {
            return sign | ((exponent as u16 + 1).min(31) << 10);
        }
    }
    sign | (exponent as u16) << 10 | rounded as u16
}

fn f16_bits_to_f32(value: u16) -> f32 {
    let sign = u32::from(value & 0x8000) << 16;
    let exponent = u32::from((value >> 10) & 0x1f);
    let mantissa = u32::from(value & 0x03ff);
    let bits = match exponent {
        0 if mantissa == 0 => sign,
        0 => {
            let leading = mantissa.leading_zeros() - 22;
            let normalized = (mantissa << (leading + 1)) & 0x03ff;
            let exponent = 127_u32 - 15 - leading;
            sign | exponent << 23 | normalized << 13
        }
        31 => sign | 0x7f80_0000 | mantissa << 13,
        _ => sign | (exponent + 112) << 23 | mantissa << 13,
    };
    f32::from_bits(bits)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadbackRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadbackRequest {
    pub id: ReadbackRequestId,
    pub target: ReadbackTarget,
    pub expected_target_revision: u64,
    pub required_control_revision: u64,
    pub endpoint_generation: Handle,
    pub region: ReadbackRegion,
    pub format: ReadbackFormat,
    pub deadline_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackOutcome {
    Completed,
    CancelledBeforeDispatch,
    ShutdownBeforeDispatch,
    DeadlineExceeded,
    TargetRevisionChanged,
    EndpointRestartedBeforeDispatch,
    OutcomeUnknownOnEndpointRestart,
    OutcomeUnknownOnShutdown,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadbackResult {
    pub id: ReadbackRequestId,
    pub target: ReadbackTarget,
    pub outcome: ReadbackOutcome,
    pub target_revision: u64,
    pub row_bytes: u32,
    pub bytes: Vec<u8>,
    pub lease: Option<BufferLease>,
    pub diagnostic: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadbackDispatch {
    pub request: ReadbackRequest,
    pub staging_offset: usize,
    pub staging_bytes: usize,
    pub row_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadbackConfig {
    pub max_pending: usize,
    pub max_terminal: usize,
    pub max_width: u32,
    pub max_height: u32,
    pub max_bytes_per_request: usize,
    pub max_staging_bytes: usize,
}

impl Default for ReadbackConfig {
    fn default() -> Self {
        Self {
            max_pending: 256,
            max_terminal: 1024,
            max_width: 16_384,
            max_height: 16_384,
            max_bytes_per_request: 256 * 1024 * 1024,
            max_staging_bytes: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackError {
    Closed,
    InvalidConfig,
    WrongEngine,
    InvalidRequest,
    DuplicateRequest,
    RequestCapacity,
    TerminalCapacity,
    StagingCapacity,
    UnknownRequest,
    AlreadyDispatched,
    NotDispatched,
    StaleEndpoint,
    TargetRevisionMismatch,
    CompletionShape,
    ClockRegression,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestState {
    Pending,
    Dispatched {
        staging_offset: usize,
        staging_bytes: usize,
        row_bytes: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingReadback {
    request: ReadbackRequest,
    state: RequestState,
}

pub struct ReadbackQueue {
    engine: EngineId,
    endpoint_generation: Handle,
    config: ReadbackConfig,
    pending: BTreeMap<ReadbackRequestId, PendingReadback>,
    terminal: VecDeque<ReadbackResult>,
    staging_bytes: usize,
    staging_high_water: usize,
    free_staging: BTreeMap<usize, usize>,
    last_request: u64,
    last_clock_millis: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadbackQueueOwnerSnapshot {
    pub closed: bool,
    pub pending: usize,
    pub dispatched: usize,
    pub terminal: usize,
    pub staging_bytes: usize,
    pub staging_high_water: usize,
    pub free_staging_regions: usize,
    pub terminal_bytes: usize,
    pub terminal_leases: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadbackQueueShutdownReport {
    pub released_pending: usize,
    pub released_dispatched: usize,
    pub released_staging_bytes: usize,
    pub terminal_results: Vec<ReadbackResult>,
}

impl ReadbackQueue {
    pub fn new(
        engine: EngineId,
        endpoint_generation: Handle,
        config: ReadbackConfig,
    ) -> Result<Self, ReadbackError> {
        if !engine.is_valid()
            || !endpoint_generation.is_valid()
            || config.max_pending == 0
            || config.max_terminal == 0
            || config.max_width == 0
            || config.max_height == 0
            || config.max_bytes_per_request == 0
            || config.max_staging_bytes == 0
            || config.max_bytes_per_request > config.max_staging_bytes
        {
            return Err(ReadbackError::InvalidConfig);
        }
        Ok(Self {
            engine,
            endpoint_generation,
            config,
            pending: BTreeMap::new(),
            terminal: VecDeque::new(),
            staging_bytes: 0,
            staging_high_water: 0,
            free_staging: BTreeMap::new(),
            last_request: 0,
            last_clock_millis: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> ReadbackQueueOwnerSnapshot {
        ReadbackQueueOwnerSnapshot {
            closed: self.closed,
            pending: self.pending.len(),
            dispatched: self
                .pending
                .values()
                .filter(|pending| matches!(pending.state, RequestState::Dispatched { .. }))
                .count(),
            terminal: self.terminal.len(),
            staging_bytes: self.staging_bytes,
            staging_high_water: self.staging_high_water,
            free_staging_regions: self.free_staging.len(),
            terminal_bytes: self
                .terminal
                .iter()
                .map(|result| result.bytes.len() + result.diagnostic.len())
                .sum(),
            terminal_leases: self
                .terminal
                .iter()
                .filter(|result| result.lease.is_some())
                .count(),
        }
    }

    pub fn shutdown(&mut self) -> ReadbackQueueShutdownReport {
        if self.closed {
            return ReadbackQueueShutdownReport {
                released_pending: 0,
                released_dispatched: 0,
                released_staging_bytes: 0,
                terminal_results: Vec::new(),
            };
        }
        let snapshot = self.owner_snapshot();
        let pending = std::mem::take(&mut self.pending);
        let mut terminal_results = self.terminal.drain(..).collect::<Vec<_>>();
        terminal_results.extend(pending.into_values().map(|pending| ReadbackResult {
            id: pending.request.id,
            target: pending.request.target,
            outcome: match pending.state {
                RequestState::Pending => ReadbackOutcome::ShutdownBeforeDispatch,
                RequestState::Dispatched { .. } => ReadbackOutcome::OutcomeUnknownOnShutdown,
            },
            target_revision: pending.request.expected_target_revision,
            row_bytes: 0,
            bytes: Vec::new(),
            lease: None,
            diagnostic: Vec::new(),
        }));
        self.staging_bytes = 0;
        self.staging_high_water = 0;
        self.free_staging.clear();
        self.closed = true;
        ReadbackQueueShutdownReport {
            released_pending: snapshot.pending - snapshot.dispatched,
            released_dispatched: snapshot.dispatched,
            released_staging_bytes: snapshot.staging_bytes,
            terminal_results,
        }
    }

    pub fn submit(&mut self, request: ReadbackRequest) -> Result<(), ReadbackError> {
        self.ensure_open()?;
        self.validate_request(request)?;
        if request.id.0 <= self.last_request || self.pending.contains_key(&request.id) {
            return Err(ReadbackError::DuplicateRequest);
        }
        if self.pending.len() >= self.config.max_pending {
            return Err(ReadbackError::RequestCapacity);
        }
        self.last_request = request.id.0;
        self.pending.insert(
            request.id,
            PendingReadback {
                request,
                state: RequestState::Pending,
            },
        );
        Ok(())
    }

    pub fn dispatch(
        &mut self,
        id: ReadbackRequestId,
        current_target_revision: u64,
    ) -> Result<ReadbackDispatch, ReadbackError> {
        self.ensure_open()?;
        let pending = *self.pending.get(&id).ok_or(ReadbackError::UnknownRequest)?;
        if pending.state != RequestState::Pending {
            return Err(ReadbackError::AlreadyDispatched);
        }
        if pending.request.endpoint_generation != self.endpoint_generation {
            return Err(ReadbackError::StaleEndpoint);
        }
        if pending.request.expected_target_revision != current_target_revision {
            self.finish(
                id,
                ReadbackOutcome::TargetRevisionChanged,
                current_target_revision,
                0,
                Vec::new(),
                None,
                Vec::new(),
            )?;
            return Err(ReadbackError::TargetRevisionMismatch);
        }
        let (row_bytes, staging_bytes) = request_shape(&pending.request, &self.config)?;
        self.staging_bytes
            .checked_add(staging_bytes)
            .filter(|bytes| *bytes <= self.config.max_staging_bytes)
            .ok_or(ReadbackError::StagingCapacity)?;
        let staging_offset = self.allocate_staging(staging_bytes)?;
        self.staging_bytes += staging_bytes;
        self.pending.get_mut(&id).unwrap().state = RequestState::Dispatched {
            staging_offset,
            staging_bytes,
            row_bytes,
        };
        Ok(ReadbackDispatch {
            request: pending.request,
            staging_offset,
            staging_bytes,
            row_bytes,
        })
    }

    pub fn complete(
        &mut self,
        id: ReadbackRequestId,
        endpoint_generation: Handle,
        target_revision: u64,
        bytes: Vec<u8>,
        lease: Option<BufferLease>,
    ) -> Result<(), ReadbackError> {
        self.ensure_open()?;
        if endpoint_generation != self.endpoint_generation {
            return Err(ReadbackError::StaleEndpoint);
        }
        let pending = *self.pending.get(&id).ok_or(ReadbackError::UnknownRequest)?;
        let RequestState::Dispatched {
            staging_bytes,
            row_bytes,
            ..
        } = pending.state
        else {
            return Err(ReadbackError::NotDispatched);
        };
        if target_revision != pending.request.expected_target_revision
            || bytes.len() != staging_bytes
            || lease.is_some_and(|lease| lease.len != staging_bytes)
        {
            return Err(ReadbackError::CompletionShape);
        }
        self.finish(
            id,
            ReadbackOutcome::Completed,
            target_revision,
            row_bytes,
            bytes,
            lease,
            Vec::new(),
        )
    }

    pub fn fail(
        &mut self,
        id: ReadbackRequestId,
        diagnostic: Vec<u8>,
    ) -> Result<(), ReadbackError> {
        self.ensure_open()?;
        if diagnostic.is_empty() || diagnostic.len() > 4096 {
            return Err(ReadbackError::InvalidRequest);
        }
        let target_revision = self
            .pending
            .get(&id)
            .ok_or(ReadbackError::UnknownRequest)?
            .request
            .expected_target_revision;
        self.finish(
            id,
            ReadbackOutcome::Failed,
            target_revision,
            0,
            Vec::new(),
            None,
            diagnostic,
        )
    }

    pub fn cancel(&mut self, id: ReadbackRequestId) -> Result<(), ReadbackError> {
        self.ensure_open()?;
        let pending = *self.pending.get(&id).ok_or(ReadbackError::UnknownRequest)?;
        match pending.state {
            RequestState::Pending => self.finish(
                id,
                ReadbackOutcome::CancelledBeforeDispatch,
                pending.request.expected_target_revision,
                0,
                Vec::new(),
                None,
                Vec::new(),
            ),
            RequestState::Dispatched { .. } => Err(ReadbackError::AlreadyDispatched),
        }
    }

    pub fn advance_clock(&mut self, now_millis: u64) -> Result<usize, ReadbackError> {
        self.ensure_open()?;
        if now_millis < self.last_clock_millis {
            return Err(ReadbackError::ClockRegression);
        }
        self.last_clock_millis = now_millis;
        let expired = self
            .pending
            .values()
            .filter(|pending| {
                pending.state == RequestState::Pending
                    && pending.request.deadline_millis <= now_millis
            })
            .map(|pending| pending.request.id)
            .collect::<Vec<_>>();
        for id in &expired {
            let revision = self.pending[id].request.expected_target_revision;
            self.finish(
                *id,
                ReadbackOutcome::DeadlineExceeded,
                revision,
                0,
                Vec::new(),
                None,
                Vec::new(),
            )?;
        }
        Ok(expired.len())
    }

    pub fn restart(&mut self, generation: Handle) -> Result<usize, ReadbackError> {
        self.ensure_open()?;
        if !generation.is_valid() || generation == self.endpoint_generation {
            return Err(ReadbackError::StaleEndpoint);
        }
        let pending = self.pending.values().copied().collect::<Vec<_>>();
        if self.terminal.len().saturating_add(pending.len()) > self.config.max_terminal {
            return Err(ReadbackError::TerminalCapacity);
        }
        self.endpoint_generation = generation;
        for request in &pending {
            let outcome = match request.state {
                RequestState::Pending => ReadbackOutcome::EndpointRestartedBeforeDispatch,
                RequestState::Dispatched { .. } => ReadbackOutcome::OutcomeUnknownOnEndpointRestart,
            };
            self.finish(
                request.request.id,
                outcome,
                request.request.expected_target_revision,
                0,
                Vec::new(),
                None,
                Vec::new(),
            )?;
        }
        self.staging_bytes = 0;
        self.staging_high_water = 0;
        self.free_staging.clear();
        Ok(pending.len())
    }

    pub fn take_terminal(&mut self) -> Option<ReadbackResult> {
        self.terminal.pop_front()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn validate_request(&self, request: ReadbackRequest) -> Result<(), ReadbackError> {
        self.ensure_open()?;
        if request.id.0 == 0
            || request.target.engine != self.engine
            || !request.target.target.is_valid()
            || request.expected_target_revision == 0
            || request.endpoint_generation != self.endpoint_generation
            || request.region.width == 0
            || request.region.height == 0
            || request.region.width > self.config.max_width
            || request.region.height > self.config.max_height
            || request.deadline_millis <= self.last_clock_millis
        {
            return Err(ReadbackError::InvalidRequest);
        }
        request_shape(&request, &self.config)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        id: ReadbackRequestId,
        outcome: ReadbackOutcome,
        target_revision: u64,
        row_bytes: u32,
        bytes: Vec<u8>,
        lease: Option<BufferLease>,
        diagnostic: Vec<u8>,
    ) -> Result<(), ReadbackError> {
        if self.terminal.len() >= self.config.max_terminal {
            return Err(ReadbackError::TerminalCapacity);
        }
        let pending = self
            .pending
            .remove(&id)
            .ok_or(ReadbackError::UnknownRequest)?;
        if let RequestState::Dispatched {
            staging_offset,
            staging_bytes,
            ..
        } = pending.state
        {
            self.release_staging(staging_offset, staging_bytes);
        }
        self.terminal.push_back(ReadbackResult {
            id,
            target: pending.request.target,
            outcome,
            target_revision,
            row_bytes,
            bytes,
            lease,
            diagnostic,
        });
        Ok(())
    }

    fn allocate_staging(&mut self, bytes: usize) -> Result<usize, ReadbackError> {
        if let Some((offset, available)) = self
            .free_staging
            .iter()
            .find(|(_, available)| **available >= bytes)
            .map(|(offset, available)| (*offset, *available))
        {
            self.free_staging.remove(&offset);
            if available > bytes {
                self.free_staging.insert(offset + bytes, available - bytes);
            }
            return Ok(offset);
        }
        let offset =
            align_up(self.staging_high_water, 256).ok_or(ReadbackError::StagingCapacity)?;
        self.staging_high_water = offset
            .checked_add(bytes)
            .filter(|end| *end <= self.config.max_staging_bytes)
            .ok_or(ReadbackError::StagingCapacity)?;
        Ok(offset)
    }

    fn release_staging(&mut self, offset: usize, bytes: usize) {
        self.staging_bytes -= bytes;
        let mut start = offset;
        let mut len = bytes;
        if let Some((previous_offset, previous_len)) = self
            .free_staging
            .range(..offset)
            .next_back()
            .map(|(offset, len)| (*offset, *len))
        {
            if previous_offset + previous_len == offset {
                self.free_staging.remove(&previous_offset);
                start = previous_offset;
                len += previous_len;
            }
        }
        if let Some(next_len) = self.free_staging.remove(&(start + len)) {
            len += next_len;
        }
        if start + len == self.staging_high_water {
            self.staging_high_water = start;
        } else {
            self.free_staging.insert(start, len);
        }
    }

    fn ensure_open(&self) -> Result<(), ReadbackError> {
        if self.closed {
            Err(ReadbackError::Closed)
        } else {
            Ok(())
        }
    }
}

fn request_shape(
    request: &ReadbackRequest,
    config: &ReadbackConfig,
) -> Result<(u32, usize), ReadbackError> {
    let row = usize::try_from(request.region.width)
        .ok()
        .and_then(|width| width.checked_mul(request.format.bytes_per_pixel()))
        .ok_or(ReadbackError::InvalidRequest)?;
    let aligned_row = align_up(row, 256).ok_or(ReadbackError::InvalidRequest)?;
    let bytes = usize::try_from(request.region.height)
        .ok()
        .and_then(|height| aligned_row.checked_mul(height))
        .filter(|bytes| *bytes <= config.max_bytes_per_request)
        .ok_or(ReadbackError::InvalidRequest)?;
    let row_bytes = u32::try_from(aligned_row).map_err(|_| ReadbackError::InvalidRequest)?;
    Ok((row_bytes, bytes))
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    let mask = alignment - 1;
    value.checked_add(mask).map(|value| value & !mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(index: u32, generation: u32) -> Handle {
        Handle { index, generation }
    }

    fn request(id: u64, generation: Handle) -> ReadbackRequest {
        ReadbackRequest {
            id: ReadbackRequestId(id),
            target: ReadbackTarget {
                engine: handle(1, 1),
                target: handle(3, 1),
            },
            expected_target_revision: 4,
            required_control_revision: 5,
            endpoint_generation: generation,
            region: ReadbackRegion {
                x: 0,
                y: 0,
                width: 1,
                height: 2,
            },
            format: ReadbackFormat::Rgba8,
            deadline_millis: 100,
        }
    }

    #[test]
    fn completion_shape_is_exact_and_failure_preserves_dispatched_work_for_retry() {
        let generation = handle(2, 1);
        let mut queue =
            ReadbackQueue::new(handle(1, 1), generation, ReadbackConfig::default()).unwrap();
        queue.submit(request(1, generation)).unwrap();
        let dispatch = queue.dispatch(ReadbackRequestId(1), 4).unwrap();
        assert_eq!(dispatch.row_bytes, 256);
        assert_eq!(dispatch.staging_bytes, 512);
        assert_eq!(
            queue.complete(ReadbackRequestId(1), generation, 4, vec![0; 511], None),
            Err(ReadbackError::CompletionShape)
        );
        assert_eq!(queue.pending_count(), 1);
        queue
            .complete(ReadbackRequestId(1), generation, 4, vec![7; 512], None)
            .unwrap();
        let result = queue.take_terminal().unwrap();
        assert_eq!(result.outcome, ReadbackOutcome::Completed);
        assert_eq!(result.row_bytes, 256);
        assert_eq!(result.bytes.len(), 512);
    }

    #[test]
    fn endpoint_restart_distinguishes_undispatched_and_outcome_unknown_requests() {
        let generation = handle(2, 1);
        let mut queue =
            ReadbackQueue::new(handle(1, 1), generation, ReadbackConfig::default()).unwrap();
        queue.submit(request(1, generation)).unwrap();
        queue.submit(request(2, generation)).unwrap();
        queue.dispatch(ReadbackRequestId(2), 4).unwrap();
        assert_eq!(queue.restart(handle(2, 2)), Ok(2));
        assert_eq!(
            queue.take_terminal().unwrap().outcome,
            ReadbackOutcome::EndpointRestartedBeforeDispatch
        );
        assert_eq!(
            queue.take_terminal().unwrap().outcome,
            ReadbackOutcome::OutcomeUnknownOnEndpointRestart
        );
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn target_revision_mismatch_is_terminal_without_staging_allocation() {
        let generation = handle(2, 1);
        let mut queue =
            ReadbackQueue::new(handle(1, 1), generation, ReadbackConfig::default()).unwrap();
        queue.submit(request(1, generation)).unwrap();
        assert_eq!(
            queue.dispatch(ReadbackRequestId(1), 9),
            Err(ReadbackError::TargetRevisionMismatch)
        );
        let result = queue.take_terminal().unwrap();
        assert_eq!(result.outcome, ReadbackOutcome::TargetRevisionChanged);
        assert_eq!(result.target_revision, 9);
    }
}
