//! Rows ↔ domain values. Every stored value is re-validated by the domain on the way out,
//! so a corrupted row is reported, never acted on.

use platform_domain::{
    BlockReason, ErrorClass, LogicalName, PlannedResource, ProviderName, ProviderResourceId,
    ResourceKind, ResourceSpec, Step, StepState,
};

use crate::DbError;

/// One `operation_steps` row.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct StepRow {
    pub position: i16,
    pub kind: String,
    pub logical_name: String,
    pub provider_name: String,
    pub spec: serde_json::Value,
    pub state: String,
    pub attempts: i16,
    pub resource_id: Option<String>,
    pub adopted: Option<bool>,
    pub block_reason: Option<String>,
    pub last_error: Option<String>,
}

pub(crate) const STEP_COLUMNS: &str = "position, kind, logical_name, provider_name, spec, state, \
     attempts, resource_id, adopted, block_reason, last_error";

fn corrupt(what: &str, detail: impl std::fmt::Display) -> DbError {
    DbError::Corrupt(format!("{what}: {detail}"))
}

pub(crate) fn error_class(value: &str) -> Option<ErrorClass> {
    [
        ErrorClass::Unavailable,
        ErrorClass::Timeout,
        ErrorClass::Protocol,
        ErrorClass::Rejected,
        ErrorClass::AlreadyExists,
    ]
    .into_iter()
    .find(|c| c.as_str() == value)
}

pub(crate) fn block_reason_str(reason: BlockReason) -> &'static str {
    match reason {
        BlockReason::Conflict => "conflict",
        BlockReason::Rejected => "rejected",
    }
}

impl StepRow {
    pub(crate) fn into_step(self) -> Result<Step, DbError> {
        let kind =
            ResourceKind::parse(&self.kind).ok_or_else(|| corrupt("step kind", &self.kind))?;
        let logical =
            LogicalName::parse(&self.logical_name).map_err(|e| corrupt("logical name", e))?;
        let name =
            ProviderName::parse(&self.provider_name).map_err(|e| corrupt("provider name", e))?;
        let spec: ResourceSpec =
            serde_json::from_value(self.spec).map_err(|e| corrupt("step spec", e))?;
        if spec.kind() != kind {
            return Err(corrupt("step spec", "kind mismatch"));
        }
        let state = match self.state.as_str() {
            "pending" => StepState::Pending,
            "uncertain" => StepState::Uncertain,
            "done" => {
                let id = self
                    .resource_id
                    .as_deref()
                    .ok_or_else(|| corrupt("done step", "no resource id"))?;
                StepState::Done {
                    resource_id: ProviderResourceId::parse(id)
                        .map_err(|e| corrupt("resource id", e))?,
                    adopted: self.adopted.unwrap_or(false),
                }
            }
            "blocked" => StepState::Blocked {
                reason: match self.block_reason.as_deref() {
                    Some("conflict") => BlockReason::Conflict,
                    Some("rejected") => BlockReason::Rejected,
                    other => return Err(corrupt("block reason", format!("{other:?}"))),
                },
            },
            "gave_up" => StepState::GaveUp,
            other => return Err(corrupt("step state", other)),
        };
        Ok(Step {
            position: u16::try_from(self.position).map_err(|e| corrupt("position", e))?,
            resource: PlannedResource {
                kind,
                logical,
                name,
                spec,
            },
            state,
            attempts: u16::try_from(self.attempts).map_err(|e| corrupt("attempts", e))?,
            last_error: self.last_error.as_deref().and_then(error_class),
        })
    }
}

/// Column arrays for a batched step update (`UNNEST`).
#[derive(Debug, Default)]
pub(crate) struct StepColumns {
    pub positions: Vec<i16>,
    pub states: Vec<&'static str>,
    pub attempts: Vec<i16>,
    pub resource_ids: Vec<Option<String>>,
    pub adopted: Vec<Option<bool>>,
    pub block_reasons: Vec<Option<&'static str>>,
    pub last_errors: Vec<Option<&'static str>>,
}

impl StepColumns {
    pub(crate) fn of(steps: &[&Step]) -> Self {
        let mut columns = Self::default();
        for step in steps {
            columns
                .positions
                .push(i16::try_from(step.position).unwrap_or(i16::MAX));
            let (state, resource_id, adopted, reason) = match &step.state {
                StepState::Pending => ("pending", None, None, None),
                StepState::Uncertain => ("uncertain", None, None, None),
                StepState::Done {
                    resource_id,
                    adopted,
                } => ("done", Some(resource_id.to_string()), Some(*adopted), None),
                StepState::Blocked { reason } => {
                    ("blocked", None, None, Some(block_reason_str(*reason)))
                }
                StepState::GaveUp => ("gave_up", None, None, None),
            };
            columns.states.push(state);
            columns
                .attempts
                .push(i16::try_from(step.attempts).unwrap_or(i16::MAX));
            columns.resource_ids.push(resource_id);
            columns.adopted.push(adopted);
            columns.block_reasons.push(reason);
            columns
                .last_errors
                .push(step.last_error.map(ErrorClass::as_str));
        }
        columns
    }
}
