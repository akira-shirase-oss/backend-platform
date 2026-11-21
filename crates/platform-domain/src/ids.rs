//! Identities. Every value that reaches a provider name is validated here, so the
//! name derivation in [`crate::resource`] can never produce an ambiguous or oversized name.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Shortest and longest project id.
pub const PROJECT_ID_LEN: std::ops::RangeInclusive<usize> = 3..=20;
/// Shortest and longest logical resource name.
pub const LOGICAL_NAME_LEN: std::ops::RangeInclusive<usize> = 1..=24;
/// Longest provider resource id accepted from a provider.
pub const MAX_PROVIDER_ID_LEN: usize = 128;

/// Why an identity was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    #[error("{what} must be {min} to {max} characters")]
    Length {
        what: &'static str,
        min: usize,
        max: usize,
    },
    #[error("{what} must start with a lowercase letter")]
    Start { what: &'static str },
    #[error("{what} may contain only {allowed}")]
    Charset {
        what: &'static str,
        allowed: &'static str,
    },
}

fn check_len(
    value: &str,
    what: &'static str,
    range: &std::ops::RangeInclusive<usize>,
) -> Result<(), IdError> {
    if range.contains(&value.len()) {
        Ok(())
    } else {
        Err(IdError::Length {
            what,
            min: *range.start(),
            max: *range.end(),
        })
    }
}

/// A project id: `[a-z][a-z0-9]{2,19}`. It carries no `_` or `-`, which is what makes
/// the provider names derived from it unambiguous (see [`crate::resource::provider_name`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectId(String);

impl ProjectId {
    /// Validates a project id.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        const WHAT: &str = "project id";
        // ATTACK(id-len)
        check_len(value, WHAT, &PROJECT_ID_LEN)?;
        if !value.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(IdError::Start { what: WHAT });
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(IdError::Charset {
                what: WHAT,
                allowed: "lowercase letters and digits",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ProjectId> for String {
    fn from(value: ProjectId) -> Self {
        value.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A resource's name inside its project: `[a-z][a-z0-9]*(_[a-z0-9]+)*`, 1–24 characters.
/// No `-`, no leading, trailing or doubled `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LogicalName(String);

impl LogicalName {
    /// Validates a logical name.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        const WHAT: &str = "resource name";
        // ATTACK(name-len)
        check_len(value, WHAT, &LOGICAL_NAME_LEN)?;
        if !value.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(IdError::Start { what: WHAT });
        }
        // ATTACK(name-charset)
        let charset_ok = value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        let underscores_ok = !value.ends_with('_') && !value.contains("__");
        if !(charset_ok && underscores_ok) {
            return Err(IdError::Charset {
                what: WHAT,
                allowed: "lowercase letters, digits and single inner underscores",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LogicalName {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<LogicalName> for String {
    fn from(value: LogicalName) -> Self {
        value.0
    }
}

impl fmt::Display for LogicalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An operation's id. Generated outside the domain (UUIDv7); also the tag written onto
/// every resource the operation creates (D-016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(pub Uuid);

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The id a provider gave a resource: opaque, 1–128 printable ASCII characters, checked
/// on the way in because it comes from outside.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderResourceId(String);

impl ProviderResourceId {
    /// Validates an id returned by a provider.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        const WHAT: &str = "provider resource id";
        // ATTACK(provider-id-len)
        check_len(value, WHAT, &(1..=MAX_PROVIDER_ID_LEN))?;
        if !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(IdError::Charset {
                what: WHAT,
                allowed: "printable ASCII without spaces",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProviderResourceId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ProviderResourceId> for String {
    fn from(value: ProviderResourceId) -> Self {
        value.0
    }
}

impl fmt::Display for ProviderResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ids() {
        for ok in ["abc", "shop2", "a0000000000000000000"] {
            assert!(ProjectId::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "ab",
            "a00000000000000000000",
            "1abc",
            "Abc",
            "a_b",
            "a-b",
            "ab c",
            "abç",
        ] {
            assert!(ProjectId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn logical_names() {
        for ok in ["main", "a", "files_2", "a_b_c", "x23456789012345678901234"] {
            assert!(LogicalName::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "_a",
            "a_",
            "a__b",
            "a-b",
            "Main",
            "9a",
            "x234567890123456789012345",
        ] {
            assert!(LogicalName::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn provider_ids_are_bounded_and_printable() {
        assert!(ProviderResourceId::parse("db-0001").is_ok());
        assert!(ProviderResourceId::parse("").is_err());
        assert!(ProviderResourceId::parse("has space").is_err());
        assert!(ProviderResourceId::parse(&"x".repeat(129)).is_err());
    }

    #[test]
    fn serde_validates_on_the_way_in() {
        let bad: Result<ProjectId, _> = serde_json::from_str("\"Not-Valid\"");
        assert!(bad.is_err());
        let good: ProjectId = serde_json::from_str("\"shop\"").unwrap();
        assert_eq!(good.as_str(), "shop");
    }
}
