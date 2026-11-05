//! Closed-vocabulary role lookup diagnostics. Never stores provider error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleLookupFailure {
    ServiceUnavailable,
    RequestRejected,
    AccountStatus,
    SearchOperation,
    SearchResult,
    CatalogIdentity,
    SearchAccount,
    CreditsOperation,
    CreditsAccount,
    CreditsBinding,
    AccountRecheck,
    AccountChanged,
}
impl RoleLookupFailure {
    pub(crate) fn stage(self) -> &'static str {
        match self {
            Self::ServiceUnavailable => "service",
            Self::RequestRejected => "request",
            Self::AccountStatus => "account_status",
            Self::SearchOperation | Self::SearchResult => "exact_search",
            Self::CatalogIdentity => "catalog_identity",
            Self::SearchAccount => "search_account",
            Self::CreditsOperation => "credits_operation",
            Self::CreditsAccount => "credits_account",
            Self::CreditsBinding => "credits_binding",
            Self::AccountRecheck | Self::AccountChanged => "account_recheck",
        }
    }
    pub(crate) fn outcome(self) -> &'static str {
        match self {
            Self::ServiceUnavailable => "service_unavailable",
            Self::RequestRejected => "request_rejected",
            Self::SearchResult => "invalid_result",
            Self::CatalogIdentity => "identity_mismatch",
            Self::SearchAccount | Self::CreditsAccount | Self::AccountChanged => "account_mismatch",
            Self::CreditsBinding => "binding_rejected",
            Self::AccountStatus
            | Self::SearchOperation
            | Self::CreditsOperation
            | Self::AccountRecheck => "operation_failed",
        }
    }
}
