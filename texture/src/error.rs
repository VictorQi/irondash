use std::fmt::Display;

// ============================================================================
// F-01: Registration Error Normalization
// ============================================================================
// This enum allows higher-level code to distinguish between different
// registration failure modes without platform-specific string matching.
//
// See: code-base/IRONDASH_FORK_WORKBOOK.md §7 (F-01)

/// Distinguishes registration failure modes for clearer error handling.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegistrationFailureMode {
    /// Called from wrong thread (not Platform Thread)
    InvalidThread,
    /// Engine handle is invalid or engine was already destroyed
    InvalidHandle,
    /// Native registration failed (e.g., registerTexture: returned error)
    NativeRegistrationFailed,
    /// Duplicate registration attempt (platform-specific)
    DuplicateRegistration,
}

// ============================================================================

#[derive(Debug)]
pub enum Error {
    /// Engine for this handle does not exist.
    EngineContextError(irondash_engine_context::Error),

    /// Texture registration failed with specific failure mode.
    TextureRegistrationFailed {
        mode: RegistrationFailureMode,
        detail: Option<String>,
    },

    /// Texture operation failed (e.g., mark_frame_available on destroyed texture)
    TextureOperationFailed {
        detail: Option<String>,
    },

    #[cfg(target_os = "android")]
    JNIError(jni::errors::Error),
}

impl Error {
    /// Helper to create InvalidThread error
    pub fn invalid_thread() -> Self {
        Error::TextureRegistrationFailed {
            mode: RegistrationFailureMode::InvalidThread,
            detail: Some("Texture registration must happen on Platform Thread".into()),
        }
    }

    /// Helper to create InvalidHandle error
    pub fn invalid_handle() -> Self {
        Error::TextureRegistrationFailed {
            mode: RegistrationFailureMode::InvalidHandle,
            detail: Some("Engine handle is invalid or engine was destroyed".into()),
        }
    }

    /// Helper to create NativeRegistrationFailed error
    pub fn native_registration_failed(detail: Option<String>) -> Self {
        Error::TextureRegistrationFailed {
            mode: RegistrationFailureMode::NativeRegistrationFailed,
            detail,
        }
    }

    /// Helper to create DuplicateRegistration error
    pub fn duplicate_registration() -> Self {
        Error::TextureRegistrationFailed {
            mode: RegistrationFailureMode::DuplicateRegistration,
            detail: Some("Duplicate texture registration for same source".into()),
        }
    }

    /// Helper to create TextureOperationFailed error
    pub fn texture_operation_failed(detail: impl Into<String>) -> Self {
        Error::TextureOperationFailed {
            detail: Some(detail.into()),
        }
    }

    /// Returns the failure mode if this is a TextureRegistrationFailed error
    pub fn registration_failure_mode(&self) -> Option<RegistrationFailureMode> {
        match self {
            Error::TextureRegistrationFailed { mode, .. } => Some(*mode),
            _ => None,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EngineContextError(e) => e.fmt(f),
            Error::TextureRegistrationFailed { mode, detail } => {
                write!(f, "texture registration failed: {:?}", mode)?;
                if let Some(detail) = detail {
                    write!(f, " ({})", detail)?;
                }
                Ok(())
            }
            Error::TextureOperationFailed { detail } => {
                write!(f, "texture operation failed")?;
                if let Some(detail) = detail {
                    write!(f, " ({})", detail)?;
                }
                Ok(())
            }
            #[cfg(target_os = "android")]
            Error::JNIError(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<irondash_engine_context::Error> for Error {
    fn from(err: irondash_engine_context::Error) -> Self {
        // Map engine context errors to appropriate texture errors
        match &err {
            irondash_engine_context::Error::InvalidThread => Error::invalid_thread(),
            irondash_engine_context::Error::InvalidHandle => Error::invalid_handle(),
            _ => Error::EngineContextError(err),
        }
    }
}

#[cfg(target_os = "android")]
impl From<jni::errors::Error> for Error {
    fn from(err: jni::errors::Error) -> Self {
        Error::JNIError(err)
    }
}
