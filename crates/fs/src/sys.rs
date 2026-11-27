//! Platform calls: file locks, free disk space, privileges.
//! The only unsafe code of the crate.

use std::fs::File;
use std::io;

/// Tries to take an exclusive lock on the whole file without waiting.
/// Returns `Ok(false)` if another process holds a conflicting lock.
#[cfg(unix)]
pub(crate) fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    // Open file description locks conflict with the POSIX locks Java uses and,
    // unlike them, are not released when another descriptor of the file closes.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const CMD: libc::c_int = libc::F_OFD_SETLK;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const CMD: libc::c_int = libc::F_SETLK;

    // SAFETY: `flock` is plain data; zero is a valid value for every field.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 0;
    lock.l_len = 0; // to the end of the file, however large it grows
    lock.l_pid = 0; // required for OFD locks
    // SAFETY: the descriptor is open for the lifetime of `file`, and `lock` is a valid `flock`.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), CMD, &lock) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EACCES | libc::EAGAIN) => Ok(false),
        _ => Err(err),
    }
}

#[cfg(windows)]
pub(crate) fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // SAFETY: `OVERLAPPED` is plain data; zero means offset 0.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the lifetime of `file`; the whole range is locked.
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == ERROR_LOCK_VIOLATION as i32 || code == ERROR_IO_PENDING as i32 => {
            Ok(false)
        }
        _ => Err(err),
    }
}

/// Bytes available to this user on the filesystem that holds `path`.
#[cfg(unix)]
pub(crate) fn available_space(path: &std::path::Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    // SAFETY: `statvfs` is plain data; zero is a valid value.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid C string and `stat` a valid out pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // The field types differ between platforms (u32 on some, u64 on others).
    #[allow(clippy::useless_conversion)]
    Ok(u64::from(stat.f_bavail).saturating_mul(u64::from(stat.f_frsize)))
}

#[cfg(windows)]
pub(crate) fn available_space(path: &std::path::Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated; the out pointers are valid or null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

/// Whether the process runs as root (and may change file owners).
#[cfg(unix)]
pub(crate) fn is_root() -> bool {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The effective user ID of the process.
#[cfg(unix)]
pub(crate) fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}
