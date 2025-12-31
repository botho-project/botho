//! Secure Memory Management
//!
//! Provides memory locking functionality to prevent sensitive data (like
//! mnemonic phrases) from being swapped to disk. This is a defense-in-depth
//! measure.
//!
//! ## Platform Support
//!
//! - **Unix**: Uses `mlock()` to lock memory pages
//! - **Windows**: Uses `VirtualLock()` to lock memory pages
//! - **Other**: Logs warning and continues without locking
//!
//! ## Security Notes
//!
//! - Memory locking may require elevated permissions on some systems
//! - Failures are logged but don't prevent operation (graceful degradation)
//! - This complements `zeroize` - mlock prevents swap, zeroize clears on drop

use std::ptr::NonNull;

/// Result of a memory lock operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockResult {
    /// Memory was successfully locked
    Locked,
    /// Memory locking failed (logged warning, continuing without lock)
    Failed,
    /// Memory locking not supported on this platform
    Unsupported,
}

/// Lock a memory region to prevent it from being swapped to disk.
///
/// # Arguments
/// * `ptr` - Pointer to the start of the memory region
/// * `len` - Length of the memory region in bytes
///
/// # Returns
/// * `LockResult::Locked` if successful
/// * `LockResult::Failed` if the operation failed (warning logged)
/// * `LockResult::Unsupported` on unsupported platforms
///
/// # Safety
/// The caller must ensure that `ptr` points to valid memory of at least `len`
/// bytes.
pub unsafe fn mlock(ptr: NonNull<u8>, len: usize) -> LockResult {
    if len == 0 {
        return LockResult::Locked;
    }

    #[cfg(unix)]
    {
        mlock_unix(ptr, len)
    }

    #[cfg(windows)]
    {
        mlock_windows(ptr, len)
    }

    #[cfg(not(any(unix, windows)))]
    {
        tracing::warn!(
            "Memory locking not supported on this platform - mnemonic may be swapped to disk"
        );
        LockResult::Unsupported
    }
}

/// Unlock a previously locked memory region.
///
/// # Arguments
/// * `ptr` - Pointer to the start of the memory region
/// * `len` - Length of the memory region in bytes
///
/// # Safety
/// The caller must ensure that `ptr` points to valid memory of at least `len`
/// bytes, and that this memory was previously locked with `mlock()`.
pub unsafe fn munlock(ptr: NonNull<u8>, len: usize) {
    if len == 0 {
        return;
    }

    #[cfg(unix)]
    {
        munlock_unix(ptr, len);
    }

    #[cfg(windows)]
    {
        munlock_windows(ptr, len);
    }

    // On unsupported platforms, this is a no-op
}

// ============================================================================
// Unix Implementation
// ============================================================================

#[cfg(unix)]
unsafe fn mlock_unix(ptr: NonNull<u8>, len: usize) -> LockResult {
    let result = libc::mlock(ptr.as_ptr() as *const libc::c_void, len);

    if result == 0 {
        tracing::debug!("Successfully locked {} bytes of memory", len);
        LockResult::Locked
    } else {
        // Use std::io::Error to get errno portably across Unix variants
        let error = std::io::Error::last_os_error();
        let errno = error.raw_os_error().unwrap_or(0);
        let error_msg = match errno {
            libc::ENOMEM => "insufficient memory or exceeds RLIMIT_MEMLOCK",
            libc::EPERM => "insufficient permissions (may need CAP_IPC_LOCK)",
            libc::EINVAL => "invalid address range",
            _ => "unknown error",
        };
        tracing::warn!(
            "Failed to lock memory: {} (errno {}). Mnemonic may be swapped to disk.",
            error_msg,
            errno
        );
        LockResult::Failed
    }
}

#[cfg(unix)]
unsafe fn munlock_unix(ptr: NonNull<u8>, len: usize) {
    let result = libc::munlock(ptr.as_ptr() as *const libc::c_void, len);

    if result != 0 {
        // Log but don't fail - memory is being dropped anyway
        tracing::debug!("munlock returned non-zero (likely already unlocked)");
    }
}

// ============================================================================
// Windows Implementation
// ============================================================================

#[cfg(windows)]
unsafe fn mlock_windows(ptr: NonNull<u8>, len: usize) -> LockResult {
    use windows::Win32::System::Memory::VirtualLock;

    let result = VirtualLock(ptr.as_ptr() as *const std::ffi::c_void, len);

    if result.is_ok() {
        tracing::debug!("Successfully locked {} bytes of memory", len);
        LockResult::Locked
    } else {
        // Get last error for diagnostics
        let error = windows::core::Error::from_win32();
        tracing::warn!(
            "Failed to lock memory: {}. Mnemonic may be swapped to disk.",
            error
        );
        LockResult::Failed
    }
}

#[cfg(windows)]
unsafe fn munlock_windows(ptr: NonNull<u8>, len: usize) {
    use windows::Win32::System::Memory::VirtualUnlock;

    let result = VirtualUnlock(ptr.as_ptr() as *const std::ffi::c_void, len);

    if result.is_err() {
        // Log but don't fail - memory is being dropped anyway
        tracing::debug!("VirtualUnlock failed (likely already unlocked)");
    }
}

/// A wrapper that locks memory on creation and unlocks on drop.
///
/// This provides RAII-style memory locking for sensitive data like mnemonics.
/// Memory locking failures are handled gracefully (logged, but operation
/// continues).
#[derive(Debug)]
pub struct LockedRegion {
    ptr: NonNull<u8>,
    len: usize,
    was_locked: bool,
}

impl LockedRegion {
    /// Create a new locked region for the given memory.
    ///
    /// # Safety
    /// - `ptr` must point to valid memory of at least `len` bytes
    /// - The memory must remain valid for the lifetime of this `LockedRegion`
    /// - The caller must ensure the memory is not freed while this region
    ///   exists
    pub unsafe fn new(ptr: NonNull<u8>, len: usize) -> Self {
        let lock_result = mlock(ptr, len);
        Self {
            ptr,
            len,
            was_locked: lock_result == LockResult::Locked,
        }
    }

    /// Returns true if the memory was successfully locked.
    pub fn is_locked(&self) -> bool {
        self.was_locked
    }
}

impl Drop for LockedRegion {
    fn drop(&mut self) {
        if self.was_locked {
            // SAFETY: We only unlock memory that we successfully locked,
            // and the memory is still valid (we're being dropped before the data).
            unsafe {
                munlock(self.ptr, self.len);
            }
        }
    }
}

// LockedRegion is Send+Sync because it only holds a pointer and metadata,
// and the actual memory management is handled by the underlying OS.
// SAFETY: The pointer is not dereferenced, only passed to OS memory locking
// functions.
unsafe impl Send for LockedRegion {}
unsafe impl Sync for LockedRegion {}

/// Lock the memory backing a string.
///
/// Returns a `LockedRegion` that will unlock the memory when dropped.
/// The returned region must be dropped before the string is deallocated.
///
/// # Safety
/// The caller must ensure the string outlives the returned `LockedRegion`.
pub unsafe fn lock_string(s: &str) -> LockedRegion {
    if let Some(ptr) = NonNull::new(s.as_ptr() as *mut u8) {
        LockedRegion::new(ptr, s.len())
    } else {
        // Empty string, nothing to lock
        LockedRegion {
            ptr: NonNull::dangling(),
            len: 0,
            was_locked: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_unlock_basic() {
        let data = vec![0u8; 4096];
        let ptr = NonNull::new(data.as_ptr() as *mut u8).unwrap();

        // SAFETY: data is valid for the duration of this test
        unsafe {
            let result = mlock(ptr, data.len());
            // Result depends on platform permissions, just verify it doesn't panic
            assert!(matches!(
                result,
                LockResult::Locked | LockResult::Failed | LockResult::Unsupported
            ));

            // Unlock if it was locked
            if result == LockResult::Locked {
                munlock(ptr, data.len());
            }
        }
    }

    #[test]
    fn test_locked_region_raii() {
        let data = vec![0u8; 4096];
        let ptr = NonNull::new(data.as_ptr() as *mut u8).unwrap();

        // SAFETY: data outlives the region
        let region = unsafe { LockedRegion::new(ptr, data.len()) };

        // Verify it was created (whether locked or not depends on permissions)
        let _ = region.is_locked();

        // Drop should not panic regardless of lock state
        drop(region);
    }

    #[test]
    fn test_lock_string() {
        let secret = "test secret mnemonic phrase";

        // SAFETY: secret outlives the region
        let region = unsafe { lock_string(secret) };

        // Verify it was created
        let _ = region.is_locked();

        // Drop should not panic
        drop(region);
    }

    #[test]
    fn test_empty_lock() {
        let ptr = NonNull::dangling();

        // SAFETY: Length 0 means no memory is accessed
        unsafe {
            let result = mlock(ptr, 0);
            assert_eq!(result, LockResult::Locked);
            munlock(ptr, 0);
        }
    }

    #[test]
    fn test_lock_result_variants() {
        // Ensure all variants exist and are distinct
        assert_ne!(LockResult::Locked, LockResult::Failed);
        assert_ne!(LockResult::Locked, LockResult::Unsupported);
        assert_ne!(LockResult::Failed, LockResult::Unsupported);
    }
}
