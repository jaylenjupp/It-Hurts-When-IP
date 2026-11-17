// Errors the helper reports back to the client. Every refusal carries a stable
// machine-readable code (sent as "code" in protocol v2) plus a plain message
// for people (sent as "error" in both v1 and v2).

/// Stable error codes for protocol v2. Never rename one: the app keys on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrCode {
    /// A required request field is absent or null.
    MissingField,
    /// A request field has the wrong JSON type (e.g. a number, not a string).
    InvalidField,
    /// The service name is empty, too long or contains control characters.
    InvalidService,
    /// No service with exactly that name exists.
    UnknownService,
    /// The service exists but is disabled.
    ServiceDisabled,
    InvalidIp,
    InvalidSubnet,
    InvalidGateway,
    /// networksetup could not be run or reported a failure.
    CommandFailed,
    UnknownCommand,
    /// The original settings could not be recorded, so nothing was changed.
    JournalFailed,
    /// restore_service: the journal can't be read or parsed.
    JournalUnusable,
    /// restore_service: no original settings are recorded for the service.
    NotJournaled,
    /// restore_service: the recorded original is neither DHCP nor Manual, or
    /// its recorded addresses don't pass validation.
    UnsupportedOriginal,
    /// restore_service: the write ran but the service doesn't read back as
    /// its original settings.
    VerifyFailed,
}

impl ErrCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrCode::MissingField => "missing_field",
            ErrCode::InvalidField => "invalid_field",
            ErrCode::InvalidService => "invalid_service",
            ErrCode::UnknownService => "unknown_service",
            ErrCode::ServiceDisabled => "service_disabled",
            ErrCode::InvalidIp => "invalid_ip",
            ErrCode::InvalidSubnet => "invalid_subnet",
            ErrCode::InvalidGateway => "invalid_gateway",
            ErrCode::CommandFailed => "command_failed",
            ErrCode::UnknownCommand => "unknown_command",
            ErrCode::JournalFailed => "journal_failed",
            ErrCode::JournalUnusable => "journal_unusable",
            ErrCode::NotJournaled => "not_journaled",
            ErrCode::UnsupportedOriginal => "unsupported_original",
            ErrCode::VerifyFailed => "verify_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperError {
    pub code: ErrCode,
    pub message: String,
}

impl HelperError {
    pub fn new(code: ErrCode, message: impl Into<String>) -> Self {
        HelperError {
            code,
            message: message.into(),
        }
    }
}
