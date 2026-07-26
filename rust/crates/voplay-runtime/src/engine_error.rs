use voplay_protocol::{EngineId, EntityId, Handle};

use crate::{asset::AssetRef, control::StableControlRef, EngineError};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EngineErrorDomain {
    Engine,
    World,
    Protocol,
    Control,
    Asset,
    Render,
    Audio,
    Physics,
    Animation,
    Platform,
    Inspection,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EngineErrorScope {
    Operation,
    Entity,
    Asset,
    RenderView,
    Endpoint,
    Engine,
    Session,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EngineErrorSeverity {
    Info,
    Warning,
    Error,
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EngineErrorRecoverability {
    Retry,
    Resync,
    RestartEndpoint,
    RestartEngine,
    CloseEngine,
    CloseSession,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineErrorContext {
    pub engine: EngineId,
    pub app_view: Option<Handle>,
    pub render_view: Option<StableControlRef>,
    pub asset: Option<AssetRef>,
    pub entity: Option<EntityId>,
    pub endpoint: Option<Handle>,
}

impl EngineErrorContext {
    pub fn validate(self) -> bool {
        self.engine.is_valid()
            && self.app_view.is_none_or(Handle::is_valid)
            && self
                .render_view
                .is_none_or(|view| view.engine == self.engine && view.handle.is_valid())
            && self
                .asset
                .is_none_or(|asset| asset.engine == self.engine && asset.handle.is_valid())
            && self.entity.is_none_or(EntityId::is_valid)
            && self.endpoint.is_none_or(Handle::is_valid)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineErrorCause {
    pub domain: EngineErrorDomain,
    pub code: u32,
    pub operation: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineErrorReport {
    pub domain: EngineErrorDomain,
    pub code: u32,
    pub scope: EngineErrorScope,
    pub severity: EngineErrorSeverity,
    pub recoverability: EngineErrorRecoverability,
    pub operation: String,
    pub context: EngineErrorContext,
    pub causes: Vec<EngineErrorCause>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineErrorReportError {
    InvalidContext,
    InvalidOperation,
    CauseCapacity,
    InvalidCause,
}

impl EngineErrorReport {
    pub const MAX_CAUSES: usize = 16;
    pub const MAX_OPERATION_BYTES: usize = 256;

    pub fn new(
        error: EngineError,
        operation: impl Into<String>,
        context: EngineErrorContext,
    ) -> Result<Self, EngineErrorReportError> {
        if !context.validate() {
            return Err(EngineErrorReportError::InvalidContext);
        }
        let operation = operation.into();
        if !valid_operation(&operation) {
            return Err(EngineErrorReportError::InvalidOperation);
        }
        let (domain, code, scope, severity, recoverability) = classify(&error);
        Ok(Self {
            domain,
            code,
            scope,
            severity,
            recoverability,
            operation,
            context,
            causes: Vec::new(),
        })
    }

    pub fn push_cause(&mut self, cause: EngineErrorCause) -> Result<(), EngineErrorReportError> {
        if self.causes.len() == Self::MAX_CAUSES {
            return Err(EngineErrorReportError::CauseCapacity);
        }
        if cause.code == 0 || !valid_operation(&cause.operation) {
            return Err(EngineErrorReportError::InvalidCause);
        }
        self.causes.push(cause);
        Ok(())
    }
}

impl EngineError {
    pub fn report(
        self,
        operation: impl Into<String>,
        context: EngineErrorContext,
    ) -> Result<EngineErrorReport, EngineErrorReportError> {
        EngineErrorReport::new(self, operation, context)
    }
}

fn classify(
    error: &EngineError,
) -> (
    EngineErrorDomain,
    u32,
    EngineErrorScope,
    EngineErrorSeverity,
    EngineErrorRecoverability,
) {
    use EngineError::*;
    match error {
        InvalidConfig => (
            EngineErrorDomain::Engine,
            1,
            EngineErrorScope::Engine,
            EngineErrorSeverity::Fatal,
            EngineErrorRecoverability::CloseEngine,
        ),
        WrongEngine | InvalidEntity | EntityAlreadyExists | EntityNotFound => (
            EngineErrorDomain::World,
            2,
            EngineErrorScope::Entity,
            EngineErrorSeverity::Error,
            EngineErrorRecoverability::Resync,
        ),
        StaleChannel
        | DuplicateCommit
        | BaseRevisionMismatch
        | InvalidRevision
        | DuplicateEntityOperation
        | EventSequence => (
            EngineErrorDomain::Protocol,
            3,
            EngineErrorScope::Endpoint,
            EngineErrorSeverity::Error,
            EngineErrorRecoverability::Resync,
        ),
        ControlRevisionUnavailable => (
            EngineErrorDomain::Control,
            4,
            EngineErrorScope::Endpoint,
            EngineErrorSeverity::Warning,
            EngineErrorRecoverability::Resync,
        ),
        TransactionCapacity | SnapshotCapacity | RenderObjectCapacity | EventCapacity => (
            EngineErrorDomain::Engine,
            5,
            EngineErrorScope::Operation,
            EngineErrorSeverity::Error,
            EngineErrorRecoverability::Retry,
        ),
        ChannelEpochExhausted => (
            EngineErrorDomain::Protocol,
            6,
            EngineErrorScope::Endpoint,
            EngineErrorSeverity::Fatal,
            EngineErrorRecoverability::CloseEngine,
        ),
        ClockRegression => (
            EngineErrorDomain::Engine,
            7,
            EngineErrorScope::Engine,
            EngineErrorSeverity::Fatal,
            EngineErrorRecoverability::RestartEngine,
        ),
        Closed => (
            EngineErrorDomain::Engine,
            8,
            EngineErrorScope::Operation,
            EngineErrorSeverity::Warning,
            EngineErrorRecoverability::CloseEngine,
        ),
    }
}

fn valid_operation(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= EngineErrorReport::MAX_OPERATION_BYTES
        && !operation.chars().any(char::is_control)
}
