//! Structured rejection helpers shared by plugin host handlers.

use nsh_engine::{HostCall, HostReply};
use steward_plugin_contract::host_api::{HostCallError, HostErrorCode};

pub(crate) trait RejectHostError {
    fn reject_host_error(self, error: HostCallError);

    fn reject_code(self, code: HostErrorCode, message: impl Into<String>)
    where
        Self: Sized,
    {
        self.reject_host_error(HostCallError::new(code, message));
    }
}

impl RejectHostError for HostCall {
    fn reject_host_error(self, error: HostCallError) {
        self.reject(error.to_wire());
    }
}

impl RejectHostError for HostReply {
    fn reject_host_error(self, error: HostCallError) {
        self.reject(error.to_wire());
    }
}
