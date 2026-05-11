//! macOS / nix-darwin impl: `pending_generation` is always `Ok(None)`  -
//! darwin-rebuild activates in-process with no booted-vs-activated delta.

use std::mem::MaybeUninit;

use anyhow::Result;
use nixfleet_proto::agent_wire::PendingGeneration;

/// `kern.boottime` formatted `<sec>.<usec>`: per-boot identity, not hardware UUID.
pub fn boot_id() -> Result<String> {
    let name = std::ffi::CString::new("kern.boottime").expect("static CStr");
    let mut tv: MaybeUninit<libc::timeval> = MaybeUninit::uninit();
    let mut size = std::mem::size_of::<libc::timeval>();
    // SAFETY: sysctlbyname is async-signal-safe; valid mut pointer + matching size.
    // We gate `assume_init` on rc == 0.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            tv.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(
            anyhow::Error::new(std::io::Error::last_os_error()).context("sysctl kern.boottime")
        );
    }
    let tv = unsafe { tv.assume_init() };
    Ok(format!("{}.{:06}", tv.tv_sec, tv.tv_usec))
}

pub fn pending_generation() -> Result<Option<PendingGeneration>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_id_returns_a_non_empty_string() {
        let id = boot_id().expect("boot_id() must succeed on darwin");
        assert!(!id.is_empty(), "boot_id() returned an empty string");
    }

    #[test]
    fn boot_id_is_stable_within_a_process() {
        let a = boot_id().unwrap();
        let b = boot_id().unwrap();
        assert_eq!(a, b, "boot_id must be stable within the running process");
    }

    #[test]
    fn pending_generation_is_always_none() {
        let p = pending_generation().unwrap();
        assert!(p.is_none(), "darwin must report no pending generation");
    }
}
