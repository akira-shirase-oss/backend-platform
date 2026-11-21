//! The copy-and-own accounts kit (Backend Platform D-036; `docs/ACCOUNTS.md`): the slice
//! every product's `SAAS_PLAN.md` repeats. A product copies this crate and
//! `migrations/0002_accounts.sql` into its own workspace and owns them: the tables live in
//! the product's database and the routes mount in the product's router. It is copied
//! code, never a shared login service (D-011).
//!
//! - invite-gated sign-up, email verification and password reset ([`mail`]: one port, an
//!   SMTP adapter to a loopback relay);
//! - sessions in an `HttpOnly`, `SameSite=Strict` cookie, stored as digests, with an idle
//!   and an absolute expiry;
//! - sign-in budgets per address and global, with a trusted path floods cannot block
//!   ([`signin`]); bounded password work ([`password`]);
//! - workspaces, members and roles ([`domain::allows`]);
//! - per-workspace API tokens, hashed at rest, identifiable by prefix, shown once;
//! - plan limits checked at admission ([`domain::admits`], under a row lock);
//! - account and workspace deletion that removes every row the kit holds.
//!
//! Product routes authorize with [`Accounts::access`].

pub mod domain;
pub mod error;
pub mod mail;
pub mod password;
mod routes;
pub mod signin;
pub mod store;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod tokens;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, header};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::{Action, PlanLimits, Role, allows};
use crate::error::AccountsError;
use crate::mail::{MailTransport, Message};
use crate::password::PasswordGate;
use crate::signin::{MailGate, MailRates, SignInGate, SignInRates};

pub use crate::routes::router;

/// The kit's settings (each has a variable in `deploy/env.example`).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Where people open links (`https://app.example`), without a trailing slash.
    pub public_url: String,
    /// The `From` address of every email.
    pub mail_from: String,
    /// `Secure` cookies with the `__Host-` prefix (false only for plain-HTTP development).
    pub cookie_secure: bool,
    /// A session's absolute lifetime.
    pub session_ttl: Duration,
    /// A session unused this long ends.
    pub session_idle: Duration,
    /// How long a verification link lives.
    pub verify_ttl: Duration,
    /// How long a reset link lives.
    pub reset_ttl: Duration,
    /// The plan's limits.
    pub plan: PlanLimits,
    /// Proxies whose `X-Forwarded-For` hop names the client.
    pub trusted_proxies: Vec<IpAddr>,
    /// Sign-in budgets.
    pub sign_in: SignInRates,
    /// Mail budgets.
    pub mail: MailRates,
    /// Argon2 jobs at once.
    pub password_concurrency: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            public_url: "http://127.0.0.1:61450".to_owned(),
            mail_from: "accounts@localhost".to_owned(),
            cookie_secure: true,
            session_ttl: Duration::from_hours(7 * 24),
            session_idle: Duration::from_hours(24),
            verify_ttl: Duration::from_hours(24),
            reset_ttl: Duration::from_hours(1),
            plan: PlanLimits::default(),
            trusted_proxies: Vec::new(),
            sign_in: SignInRates::default(),
            mail: MailRates::default(),
            password_concurrency: 2,
        }
    }
}

/// The kit's shared state. Cheap to clone.
#[derive(Clone)]
pub struct Accounts {
    inner: Arc<Inner>,
}

struct Inner {
    db: PgPool,
    settings: Settings,
    mail: Arc<dyn MailTransport>,
    sign_in: SignInGate,
    mail_gate: MailGate,
    passwords: PasswordGate,
}

impl std::fmt::Debug for Accounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

/// Who is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Principal {
    /// A signed-in person.
    Session {
        /// The account.
        user: Uuid,
    },
    /// An API token, bound to one workspace.
    Token {
        /// The token.
        token: Uuid,
        /// Its workspace.
        workspace: Uuid,
    },
}

/// How a request reached a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// A member, with their role.
    Member {
        /// The account.
        user: Uuid,
        /// Its role.
        role: Role,
    },
    /// An API token of this workspace.
    Token {
        /// The token.
        token: Uuid,
    },
}

/// Access granted to one workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    /// The workspace.
    pub workspace: Uuid,
    /// How.
    pub via: Via,
}

/// The session cookie's name with `Secure`: the `__Host-` prefix makes browsers refuse it
/// unless it is Secure, host-only and for `/`, so no sibling subdomain can set or shadow it.
pub const COOKIE_SECURE: &str = "__Host-starter_session";
/// The cookie's name on plain-HTTP development servers.
pub const COOKIE_PLAIN: &str = "starter_session";

impl Accounts {
    /// The kit over `db`, sending mail through `mail`.
    pub async fn new(
        db: PgPool,
        settings: Settings,
        mail: Arc<dyn MailTransport>,
    ) -> Result<Self, AccountsError> {
        let passwords = PasswordGate::new(settings.password_concurrency).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                sign_in: SignInGate::new(settings.sign_in),
                mail_gate: MailGate::new(settings.mail),
                db,
                settings,
                mail,
                passwords,
            }),
        })
    }

    pub(crate) fn db(&self) -> &PgPool {
        &self.inner.db
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.inner.settings
    }

    pub(crate) fn sign_in_gate(&self) -> &SignInGate {
        &self.inner.sign_in
    }

    pub(crate) fn mail_gate(&self) -> &MailGate {
        &self.inner.mail_gate
    }

    pub(crate) fn passwords(&self) -> &PasswordGate {
        &self.inner.passwords
    }

    pub(crate) fn cookie_name(&self) -> &'static str {
        if self.inner.settings.cookie_secure {
            COOKIE_SECURE
        } else {
            COOKIE_PLAIN
        }
    }

    /// Sends one message; false (logged, without content) when it failed.
    pub(crate) async fn send(&self, to: &str, subject: &'static str, body: String) -> bool {
        let message = Message {
            to: to.to_owned(),
            subject,
            body,
        };
        match self
            .inner
            .mail
            .send(&self.inner.settings.mail_from, &message)
            .await
        {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "an email could not be sent");
                false
            }
        }
    }

    /// The caller: an `Authorization: Bearer` API token, else the session cookie.
    pub async fn principal(&self, headers: &HeaderMap) -> Result<Principal, AccountsError> {
        if headers.get(header::AUTHORIZATION).is_some() {
            let token = api_token(headers).ok_or(AccountsError::Unauthenticated)?;
            // ATTACK(token-lookup)
            let (token, workspace) = store::token_workspace(self.db(), tokens::digest(token))
                .await?
                .ok_or(AccountsError::Unauthenticated)?;
            return Ok(Principal::Token { token, workspace });
        }
        let session =
            session_cookie(headers, self.cookie_name()).ok_or(AccountsError::Unauthenticated)?;
        let user = store::session_user(
            self.db(),
            tokens::digest(session),
            self.settings().session_idle,
        )
        .await?
        .ok_or(AccountsError::Unauthenticated)?;
        Ok(Principal::Session { user })
    }

    /// The signed-in person (an API token is refused: 403 `session_required`).
    pub async fn person(&self, headers: &HeaderMap) -> Result<Uuid, AccountsError> {
        match self.principal(headers).await? {
            // ATTACK(me-scoping)
            Principal::Session { user } => Ok(user),
            Principal::Token { .. } => Err(AccountsError::SessionRequired),
        }
    }

    /// Authorizes `action` on `workspace`. A member needs a role that allows it; an API
    /// token reaches only its own workspace, and only for [`Action::Read`]. Anything the
    /// caller may not see is 404, so workspace ids cannot be probed.
    pub async fn access(
        &self,
        headers: &HeaderMap,
        workspace: Uuid,
        action: Action,
    ) -> Result<Access, AccountsError> {
        // ATTACK(access-bypass)
        match self.principal(headers).await? {
            Principal::Token {
                token,
                workspace: own,
            } => {
                // ATTACK(token-scope)
                if own != workspace {
                    return Err(AccountsError::NotFound);
                }
                // ATTACK(token-read-only)
                if action != Action::Read {
                    return Err(AccountsError::SessionRequired);
                }
                Ok(Access {
                    workspace,
                    via: Via::Token { token },
                })
            }
            Principal::Session { user } => {
                let role = store::role_in(self.db(), workspace, user)
                    .await?
                    // ATTACK(membership)
                    .ok_or(AccountsError::NotFound)?;
                // ATTACK(role-allows)
                if !allows(role, action) {
                    return Err(AccountsError::Forbidden);
                }
                Ok(Access {
                    workspace,
                    via: Via::Member { user, role },
                })
            }
        }
    }
}

/// The API token from `Authorization: Bearer …` — only when exactly one
/// `Authorization` header arrives (two are ambiguous: refuse rather than silently
/// pick a winner) and it is shaped like one. Pure so it can be fuzzed.
pub fn api_token(headers: &HeaderMap) -> Option<&str> {
    let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
    let value = authorization.next()?;
    // ATTACK(dup-authorization)
    if authorization.next().is_some() {
        return None;
    }
    // ATTACK(auth-parse)
    let text = value.to_str().ok()?;
    text.strip_prefix("Bearer ")
        .filter(|t| tokens::looks_like_api_token(t))
}

/// The session id from the `Cookie` header, if exactly one pair carries the session
/// name and it is shaped like one. Two pairs with that name are ambiguous: refuse
/// outright rather than silently pick a winner (the Authorization header does the
/// same). `get_all` stays: HTTP/2 may split the cookie into several headers.
/// Pure so it can be fuzzed.
pub fn session_cookie<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    let mut pairs = headers
        .get_all(header::COOKIE)
        .iter()
        // ATTACK(cookie-parse)
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(n, _)| *n == name)
        .map(|(_, value)| value);
    let first = pairs.next()?;
    // ATTACK(dup-session-cookie)
    if pairs.next().is_some() {
        return None;
    }
    Some(first).filter(|v| tokens::looks_like_hex_secret(v))
}
