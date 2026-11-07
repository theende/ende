// Copyright 2018-2026 the Deno authors. MIT license.

//! A Windows job object that kills every process in it when its handle closes
//! (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).
//!
//! `ende desktop --hmr`/`--inspect` puts the laufey backend in one, so the
//! backend's helper processes (CEF's renderer and GPU processes, WebView2's
//! `msedgewebview2.exe`) end with the dev run. `kill_on_drop` only kills the
//! backend itself.
//!
//! - The job closes when the [`KillOnCloseJob`] is dropped, or when this
//!   process dies, however it dies, because the kernel then closes the handle.
//!   The handle isn't inheritable, so no child can keep the job open.
//! - Processes the child starts after [`KillOnCloseJob::assign`] are in the
//!   job too. The job doesn't allow breakaway, so they can't leave it.
//! - Processes started some other way, e.g. by a COM server or a service, are
//!   not in the job and outlive it.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;

use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;

/// A kill-on-close job. Dropping it kills every process still in it.
pub struct KillOnCloseJob(OwnedHandle);

impl KillOnCloseJob {
  /// Puts `child` in a new kill-on-close job. Keep the returned job for as
  /// long as the child's process tree should run.
  ///
  /// A process the child started before this call isn't in the job. The
  /// laufey backends start helpers only once they have initialized CEF or
  /// opened a WebView2 window, far later than this call after `spawn`.
  pub fn assign(child: &tokio::process::Child) -> io::Result<Self> {
    let process = child.raw_handle().ok_or_else(|| {
      io::Error::other("the child process has already been waited for")
    })?;
    let job = Self::new()?;
    // SAFETY: both handles are open: `job` owns the job handle, and tokio
    // keeps the process handle open for as long as `child` is borrowed.
    let assigned =
      unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), process) };
    if assigned == 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(job)
  }

  fn new() -> io::Result<Self> {
    // SAFETY: null security attributes (a handle that isn't inheritable) and
    // a null name (an anonymous job) are valid arguments.
    let handle =
      unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
      return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is a new, open job handle that nothing else owns.
    let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });

    // SAFETY: the struct holds only integers, so all zeroes is a valid value.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
      unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `info` outlives the call, and the length is its size.
    let set = unsafe {
      SetInformationJobObject(
        job.0.as_raw_handle(),
        JobObjectExtendedLimitInformation,
        (&raw const info).cast(),
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
      )
    };
    if set == 0 {
      return Err(io::Error::last_os_error());
    }
    Ok(job)
  }
}

#[cfg(test)]
mod tests {
  use std::process::Stdio;
  use std::time::Duration;

  use super::KillOnCloseJob;

  #[tokio::test]
  async fn dropping_the_job_kills_the_child() {
    // Runs for about 30 seconds unless it's killed.
    let mut child = tokio::process::Command::new("ping")
      .args(["-n", "31", "127.0.0.1"])
      .stdout(Stdio::null())
      .spawn()
      .unwrap();
    let job = KillOnCloseJob::assign(&child).unwrap();
    drop(job);
    tokio::time::timeout(Duration::from_secs(10), child.wait())
      .await
      .expect("the child outlived its job")
      .unwrap();
  }
}
