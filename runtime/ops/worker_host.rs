// Copyright 2018-2026 the Deno authors. MIT license.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use ende_core::CancelFuture;
use ende_core::CancelHandle;
use ende_core::DetachedBuffer;
use ende_core::FromV8;
use ende_core::JsBuffer;
use ende_core::JsRuntimeInspector;
use ende_core::ModuleSpecifier;
use ende_core::OpState;
use ende_core::op2;
use ende_permissions::ChildPermissionsArg;
use ende_permissions::PermissionsContainer;
use ende_web::Blob;
use ende_web::BlobStoreTrait;
use ende_web::JsMessageData;
use ende_web::MessagePortError;
use ende_web::RecvMessageData;
use ende_web::deserialize_js_transferables;
use log::debug;

use crate::ops::TestingFeaturesEnabled;
use crate::tokio_util::create_and_run_current_thread;
use crate::web_worker::SendableWebWorkerHandle;
use crate::web_worker::WebWorker;
use crate::web_worker::WebWorkerHandle;
use crate::web_worker::WorkerControlEvent;
use crate::web_worker::WorkerId;
use crate::web_worker::WorkerMetadata;
use crate::web_worker::WorkerThreadType;
use crate::web_worker::run_web_worker;
use crate::worker::FormatJsErrorFn;

pub const UNSTABLE_FEATURE_NAME: &str = "worker-options";

/// Default OS stack size for the thread a worker's isolate runs on.
///
/// This is what `resourceLimits.stackSizeMb` reports back to JS: without
/// setting it here the thread would get Rust's 2MB default while we told the
/// user it was 4MB. 2MB also leaves very little native headroom above V8's own JS stack limit
/// (`--stack-size`, 1MB by default), so a raised `--stack-size` overflows the
/// OS stack and aborts the process instead of raising `RangeError`.
const DEFAULT_WORKER_STACK_SIZE_MB: usize = 4;

/// V8 resource limits for worker isolates, matching Node.js `resourceLimits`.
#[derive(FromV8, Default, Clone)]
pub struct ResourceLimits {
  pub max_young_generation_size_mb: Option<usize>,
  pub max_old_generation_size_mb: Option<usize>,
  pub code_range_size_mb: Option<usize>,
  pub stack_size_mb: Option<usize>,
}

pub struct CreateWebWorkerArgs {
  pub name: String,
  pub worker_id: WorkerId,
  pub parent_permissions: PermissionsContainer,
  pub permissions: PermissionsContainer,
  pub main_module: ModuleSpecifier,
  pub worker_type: WorkerThreadType,
  pub close_on_idle: bool,
  pub maybe_worker_metadata: Option<WorkerMetadata>,
  /// Captured root blob for `main_module`; paired with `main_module` by the
  /// worker's loader/handle. Blob dependencies are intentionally resolved
  /// normally by their own URLs.
  pub maybe_main_module_blob: Option<Arc<Blob>>,
  pub resource_limits: Option<ResourceLimits>,
  pub wait_for_debugger_on_start: bool,
  pub wait_for_page_wait_for_debugger: bool,
}

pub type CreateWebWorkerCb = dyn Fn(CreateWebWorkerArgs) -> (WebWorker, SendableWebWorkerHandle)
  + Sync
  + Send;

/// A holder for callback that is used to create a new
/// WebWorker. It's a struct instead of a type alias
/// because `GothamState` used in `OpState` overrides
/// value if type aliases have the same underlying type
#[derive(Clone)]
struct CreateWebWorkerCbHolder(Arc<CreateWebWorkerCb>);

#[derive(Clone)]
struct FormatJsErrorFnHolder(Option<Arc<FormatJsErrorFn>>);

pub struct WorkerThread {
  worker_handle: WebWorkerHandle,
  cancel_handle: Rc<CancelHandle>,

  // A WorkerThread that hasn't been explicitly terminated can only be removed
  // from the WorkersTable once close messages have been received for both the
  // control and message channels. See `close_channel`.
  ctrl_closed: bool,
  message_closed: bool,
  termination_requested: bool,
}

impl WorkerThread {
  fn request_termination(&mut self) {
    self.termination_requested = true;
    self.worker_handle.clone().terminate();
  }

  fn finish_termination(self) {
    // Cancel recv ops when terminating the worker, so they don't show up as
    // pending ops.
    self.cancel_handle.cancel();
  }
}

impl Drop for WorkerThread {
  fn drop(&mut self) {
    self.worker_handle.clone().terminate();
  }
}

pub type WorkersTable = HashMap<WorkerId, WorkerThread>;

ende_core::extension!(
  ende_worker_host,
  ops = [
    op_create_worker,
    op_host_terminate_worker,
    op_host_post_message,
    op_host_recv_ctrl,
    op_host_post_message_raw,
    op_host_recv_message,
    op_host_recv_message_sync,
  ],
  options = {
    create_web_worker_cb: Arc<CreateWebWorkerCb>,
    format_js_error_fn: Option<Arc<FormatJsErrorFn>>,
  },
  state = |state, options| {
    state.put::<WorkersTable>(WorkersTable::default());

    let create_web_worker_cb_holder =
      CreateWebWorkerCbHolder(options.create_web_worker_cb);
    state.put::<CreateWebWorkerCbHolder>(create_web_worker_cb_holder);
    let format_js_error_fn_holder =
      FormatJsErrorFnHolder(options.format_js_error_fn);
    state.put::<FormatJsErrorFnHolder>(format_js_error_fn_holder);
  },
);

#[derive(FromV8)]
pub struct CreateWorkerArgs {
  has_source_code: bool,
  name: Option<String>,
  #[from_v8(serde)]
  permissions: Option<ChildPermissionsArg>,
  source_code: String,
  specifier: String,
  #[from_v8(serde)]
  worker_type: WorkerThreadType,
  close_on_idle: bool,
  resource_limits: Option<ResourceLimits>,
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum CreateWorkerError {
  #[class("DOMExceptionNotSupportedError")]
  #[error("Classic workers are not supported.")]
  ClassicWorkers,
  #[class(inherit)]
  #[error(transparent)]
  Permission(ende_permissions::ChildPermissionError),
  #[class(inherit)]
  #[error(transparent)]
  ModuleResolution(#[from] ende_core::ModuleResolutionError),
  #[class(inherit)]
  #[error(transparent)]
  MessagePort(#[from] MessagePortError),
  #[class(inherit)]
  #[error("{0}")]
  Io(#[from] std::io::Error),
}

/// Create worker as the host
#[op2(stack_trace)]
fn op_create_worker(
  state: &mut OpState,
  #[scoped] args: CreateWorkerArgs,
  #[serde] maybe_worker_metadata: Option<JsMessageData>,
) -> Result<WorkerId, CreateWorkerError> {
  let specifier = args.specifier.clone();
  let maybe_source_code = if args.has_source_code {
    Some(args.source_code.clone())
  } else {
    None
  };
  let args_name = args.name;
  let worker_type = args.worker_type;
  if let WorkerThreadType::Classic = worker_type
    && let TestingFeaturesEnabled(false) = state.borrow()
  {
    return Err(CreateWorkerError::ClassicWorkers);
  }

  if args.permissions.is_some() {
    super::check_unstable(
      state,
      UNSTABLE_FEATURE_NAME,
      "Worker.ende.permissions",
    );
  }

  let parent_permissions = state.borrow_mut::<PermissionsContainer>();
  let worker_permissions = if let Some(child_permissions_arg) = args.permissions
  {
    parent_permissions
      .create_child_permissions(child_permissions_arg)
      .map_err(CreateWorkerError::Permission)?
  } else {
    parent_permissions.clone()
  };
  let parent_permissions = parent_permissions.clone();
  let create_web_worker_cb = state.borrow::<CreateWebWorkerCbHolder>().clone();
  let format_js_error_fn = state.borrow::<FormatJsErrorFnHolder>().clone();
  let wait_for_debugger_on_start = state
    .try_borrow::<Rc<JsRuntimeInspector>>()
    .map(|inspector| inspector.should_wait_for_debugger_on_worker_start())
    .unwrap_or(false);
  let wait_for_page_wait_for_debugger = state
    .try_borrow::<Rc<JsRuntimeInspector>>()
    .map(|inspector| {
      inspector.should_wait_for_page_wait_for_debugger_on_worker_start()
    })
    .unwrap_or(false);
  let worker_id = WorkerId::new();

  let module_specifier = ende_core::resolve_url(&specifier)?;
  // Synchronously capture the root blob so a racing `URL.revokeObjectURL`
  // after `new Worker(blobUrl)` can't make the worker load fail (see #26142).
  // This anchors only the worker root; blob URL dependencies still resolve
  // through the normal blob store at load time.
  let maybe_main_module_blob = if module_specifier.scheme() == "blob" {
    let blob_store = state.borrow::<Arc<dyn BlobStoreTrait>>();
    blob_store.get_object_url(module_specifier.clone())
  } else {
    None
  };
  let worker_name = args_name.unwrap_or_default();

  let (handle_sender, handle_receiver) =
    std::sync::mpsc::sync_channel::<SendableWebWorkerHandle>(1);

  // Setup new thread. stackSizeMb from resourceLimits wins, matching Node.js
  // behavior; otherwise the isolate thread gets the default above rather than
  // Rust's smaller 2MB one.
  let stack_size_mb = args
    .resource_limits
    .as_ref()
    .and_then(|limits| limits.stack_size_mb.filter(|&v| v > 0))
    .unwrap_or(DEFAULT_WORKER_STACK_SIZE_MB);
  let thread_builder = std::thread::Builder::new()
    .name(format!("{worker_id}"))
    .stack_size(stack_size_mb * 1024 * 1024);
  let maybe_worker_metadata = if let Some(data) = maybe_worker_metadata {
    let transferables =
      deserialize_js_transferables(state, data.transferables)?;
    Some(WorkerMetadata {
      buffer: data.data,
      transferables,
    })
  } else {
    None
  };
  // Spawn it
  thread_builder.spawn(move || {
    // Any error inside this block is terminal:
    // - JS worker is useless - meaning it throws an exception and can't do anything else,
    //  all action done upon it should be noops
    // - newly spawned thread exits
    let fut = async move {
      let (worker, external_handle) =
        (create_web_worker_cb.0)(CreateWebWorkerArgs {
          name: worker_name,
          worker_id,
          parent_permissions,
          permissions: worker_permissions,
          main_module: module_specifier.clone(),
          worker_type,
          close_on_idle: args.close_on_idle,
          maybe_worker_metadata,
          maybe_main_module_blob,
          resource_limits: args.resource_limits,
          wait_for_debugger_on_start,
          wait_for_page_wait_for_debugger,
        });

      // Send thread safe handle from newly created worker to host thread
      handle_sender.send(external_handle).unwrap();
      drop(handle_sender);

      // At this point the only method of communication with host
      // is using `worker.internal_channels`.
      //
      // Host can already push messages and interact with worker.
      run_web_worker(
        worker,
        module_specifier,
        maybe_source_code,
        format_js_error_fn.0,
      )
      .await
    };

    let _ = create_and_run_current_thread(fut);

    // After the worker's tokio runtime and JsRuntime/V8 isolate have been
    // dropped, ask the system allocator to release freed memory back to the
    // OS. Without this, glibc in particular holds onto the fragmented heap
    // pages, causing RSS to remain high after many workers are created and
    // destroyed (https://github.com/denoland/deno/issues/26058).
    #[cfg(target_os = "linux")]
    {
      // SAFETY: calling libc function with no preconditions.
      unsafe {
        libc::malloc_trim(0);
      }
    }
  })?;

  // Receive WebWorkerHandle from newly created worker
  let worker_handle = handle_receiver.recv().map_err(|_| {
    std::io::Error::other(
      "Worker thread terminated unexpectedly before initialization completed",
    )
  })?;

  let worker_thread = WorkerThread {
    worker_handle: worker_handle.into(),
    cancel_handle: CancelHandle::new_rc(),
    ctrl_closed: false,
    message_closed: false,
    termination_requested: false,
  };

  // At this point all interactions with worker happen using thread
  // safe handler returned from previous function calls
  state
    .borrow_mut::<WorkersTable>()
    .insert(worker_id, worker_thread);

  Ok(worker_id)
}

#[op2]
fn op_host_terminate_worker(state: &mut OpState, #[scoped] id: WorkerId) {
  match state.borrow_mut::<WorkersTable>().entry(id) {
    std::collections::hash_map::Entry::Occupied(mut entry) => {
      entry.get_mut().request_termination();
    }
    std::collections::hash_map::Entry::Vacant(_) => {
      debug!("tried to terminate non-existent worker {}", id);
    }
  }
}

enum WorkerChannel {
  Ctrl,
  Messages,
}

/// Close a worker's channel. If this results in a worker no longer needing
/// host-side receive ops, the worker will be removed from the workers table.
fn close_channel(
  state: Rc<RefCell<OpState>>,
  id: WorkerId,
  channel: WorkerChannel,
) {
  use std::collections::hash_map::Entry;

  let mut s = state.borrow_mut();
  let workers = s.borrow_mut::<WorkersTable>();

  // `Worker.terminate()` might have been called already, meaning that we won't
  // find the worker in the table - in that case ignore.
  if let Entry::Occupied(mut entry) = workers.entry(id) {
    let remove = {
      let worker_thread = entry.get_mut();
      match channel {
        WorkerChannel::Ctrl => {
          worker_thread.ctrl_closed = true;
          worker_thread.termination_requested || worker_thread.message_closed
        }
        WorkerChannel::Messages => {
          worker_thread.message_closed = true;
          !worker_thread.termination_requested && worker_thread.ctrl_closed
        }
      }
    };

    if remove {
      entry.remove().finish_termination();
    }
  }
}

/// Get control event from guest worker as host
#[op2]
#[serde]
async fn op_host_recv_ctrl(
  state: Rc<RefCell<OpState>>,
  #[scoped] id: WorkerId,
) -> WorkerControlEvent {
  let (worker_handle, cancel_handle) = {
    let state = state.borrow();
    let workers_table = state.borrow::<WorkersTable>();
    let maybe_handle = workers_table.get(&id);
    if let Some(handle) = maybe_handle {
      (handle.worker_handle.clone(), handle.cancel_handle.clone())
    } else {
      // If handle was not found it means worker has already shutdown
      return WorkerControlEvent::Close(0);
    }
  };

  let maybe_event = worker_handle
    .get_control_event()
    .or_cancel(cancel_handle)
    .await;
  match maybe_event {
    Ok(Some(event)) => {
      // Terminal error or close means that worker should be removed from worker table.
      match &event {
        WorkerControlEvent::TerminalError(..)
        | WorkerControlEvent::Close(_) => {
          close_channel(state, id, WorkerChannel::Ctrl);
        }
      }
      event
    }
    Ok(None) => {
      // If there was no event from worker it means it has already been closed.
      close_channel(state, id, WorkerChannel::Ctrl);
      WorkerControlEvent::Close(0)
    }
    Err(_) => {
      // The worker was terminated.
      WorkerControlEvent::Close(0)
    }
  }
}

#[op2]
async fn op_host_recv_message(
  state: Rc<RefCell<OpState>>,
  #[scoped] id: WorkerId,
) -> Result<Option<RecvMessageData>, MessagePortError> {
  let (worker_handle, cancel_handle) = {
    let s = state.borrow();
    let workers_table = s.borrow::<WorkersTable>();
    let maybe_handle = workers_table.get(&id);
    if let Some(handle) = maybe_handle {
      (handle.worker_handle.clone(), handle.cancel_handle.clone())
    } else {
      // If handle was not found it means worker has already shutdown
      return Ok(None);
    }
  };

  let ret = worker_handle
    .port
    .recv(state.clone())
    .or_cancel(cancel_handle)
    .await;
  match ret {
    Ok(Ok(ret)) => {
      if ret.is_none() {
        close_channel(state, id, WorkerChannel::Messages);
      }
      Ok(ret)
    }
    Ok(Err(err)) => Err(err),
    Err(_) => {
      // The worker was terminated.
      Ok(None)
    }
  }
}

#[op2]
fn op_host_recv_message_sync(
  state: &mut OpState,
  #[scoped] id: WorkerId,
) -> Result<Option<JsMessageData>, MessagePortError> {
  let worker_handle = {
    let workers_table = state.borrow::<WorkersTable>();
    match workers_table.get(&id) {
      Some(handle) => handle.worker_handle.clone(),
      None => return Ok(None),
    }
  };
  worker_handle.port.try_recv_sync(state)
}

/// Post message to guest worker as host
#[op2]
fn op_host_post_message(
  state: &mut OpState,
  #[scoped] id: WorkerId,
  #[serde] data: JsMessageData,
) -> Result<(), MessagePortError> {
  if let Some(worker_thread) = state.borrow::<WorkersTable>().get(&id) {
    debug!("post message to worker {}", id);
    let worker_handle = worker_thread.worker_handle.clone();
    worker_handle.port.send(state, data)?;
  } else {
    debug!("tried to post message to non-existent worker {}", id);
  }
  Ok(())
}

/// Fast-path post: takes a pre-serialized buffer directly, bypassing
/// the JsMessageData serde overhead. Only for messages with no transferables.
#[op2]
fn op_host_post_message_raw(
  state: &mut OpState,
  #[scoped] id: WorkerId,
  #[buffer(detach)] data: JsBuffer,
) -> Result<(), MessagePortError> {
  if let Some(worker_thread) = state.borrow::<WorkersTable>().get(&id) {
    let worker_handle = worker_thread.worker_handle.clone();
    let detached = DetachedBuffer::from_v8slice(data.into_parts());
    if let Some(tx) = &*worker_handle.port.tx.borrow() {
      tx.send((detached, vec![])).ok();
    }
  }
  Ok(())
}
