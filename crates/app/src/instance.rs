//! Process-wide single-instance ownership and secondary-launch notification.

use std::io;

pub(crate) enum InstanceClaim {
    Primary(InstanceGuard),
    Secondary,
}

pub(crate) fn claim() -> io::Result<InstanceClaim> {
    imp::claim()
}

#[cfg(target_os = "windows")]
pub(crate) use imp::InstanceGuard;

#[cfg(not(target_os = "windows"))]
pub(crate) use imp::InstanceGuard;

#[cfg(target_os = "windows")]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    };

    use super::InstanceClaim;

    const INSTANCE_MUTEX_NAME: &str = "Local\\com.hiaspirin.steward.single-instance.v1";
    const ACTIVATION_EVENT_NAME: &str = "Local\\com.hiaspirin.steward.activate.v1";

    pub(crate) struct InstanceGuard {
        mutex: HANDLE,
        activation: Option<HANDLE>,
    }

    impl InstanceGuard {
        /// Consume a pending request from a secondary launch. The named event
        /// is auto-reset, so multiple rapid launches collapse into one summon.
        pub(crate) fn take_activation_request(&self) -> bool {
            self.activation
                .is_some_and(|event| unsafe { WaitForSingleObject(event, 0) == WAIT_OBJECT_0 })
        }
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(event) = self.activation {
                    CloseHandle(event);
                }
                CloseHandle(self.mutex);
            }
        }
    }

    pub(crate) fn claim() -> io::Result<InstanceClaim> {
        claim_named(INSTANCE_MUTEX_NAME, ACTIVATION_EVENT_NAME)
    }

    fn claim_named(mutex_name: &str, activation_name: &str) -> io::Result<InstanceClaim> {
        let mutex =
            unsafe { CreateMutexW(std::ptr::null(), 0, wide(mutex_name).as_slice().as_ptr()) };
        if mutex.is_null() {
            return Err(io::Error::last_os_error());
        }

        if io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
            unsafe {
                CloseHandle(mutex);
            }
            signal_activation(activation_name);
            return Ok(InstanceClaim::Secondary);
        }

        let activation = unsafe {
            CreateEventW(
                std::ptr::null(),
                0,
                0,
                wide(activation_name).as_slice().as_ptr(),
            )
        };
        Ok(InstanceClaim::Primary(InstanceGuard {
            mutex,
            activation: (!activation.is_null()).then_some(activation),
        }))
    }

    fn signal_activation(name: &str) {
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide(name).as_slice().as_ptr()) };
        if event.is_null() {
            return;
        }
        unsafe {
            SetEvent(event);
            CloseHandle(event);
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicU32, Ordering};

        use super::*;

        static TEST_ID: AtomicU32 = AtomicU32::new(0);

        fn names() -> (String, String) {
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let prefix = format!(
                "Local\\com.hiaspirin.steward.test.{}.{}",
                std::process::id(),
                id
            );
            (format!("{prefix}.mutex"), format!("{prefix}.event"))
        }

        #[test]
        fn secondary_claim_notifies_the_primary_instance() {
            let (mutex, event) = names();
            let InstanceClaim::Primary(primary) = claim_named(&mutex, &event).unwrap() else {
                panic!("first claim should own the mutex");
            };

            assert!(matches!(
                claim_named(&mutex, &event).unwrap(),
                InstanceClaim::Secondary
            ));
            assert!(primary.take_activation_request());
            assert!(!primary.take_activation_request());
        }

        #[test]
        fn mutex_is_released_when_the_primary_instance_exits() {
            let (mutex, event) = names();
            let first = claim_named(&mutex, &event).unwrap();
            assert!(matches!(first, InstanceClaim::Primary(_)));
            drop(first);

            assert!(matches!(
                claim_named(&mutex, &event).unwrap(),
                InstanceClaim::Primary(_)
            ));
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::io;

    use super::InstanceClaim;

    pub(crate) struct InstanceGuard;

    impl InstanceGuard {
        pub(crate) fn take_activation_request(&self) -> bool {
            false
        }
    }

    pub(crate) fn claim() -> io::Result<InstanceClaim> {
        Ok(InstanceClaim::Primary(InstanceGuard))
    }
}
