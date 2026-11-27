// Copyright 2018-2026 the Deno authors. MIT license.

(function () {
const { core, primordials } = __bootstrap;
const {
  op_create_extension_worker,
  op_create_sandbox_worker,
  op_create_worker,
  op_host_post_message,
  op_host_post_message_raw,
  op_host_recv_ctrl,
  op_host_recv_message,
  op_host_recv_message_sync,
  op_host_terminate_worker,
} = core.ops;
const {
  ArrayIsArray,
  ArrayPrototypeFilter,
  ArrayPrototypeJoin,
  ArrayPrototypePush,
  Error,
  EvalError,
  MapPrototypeDelete,
  MapPrototypeForEach,
  MapPrototypeGet,
  MapPrototypeSet,
  ObjectGetPrototypeOf,
  ObjectHasOwn,
  ObjectPrototype,
  ObjectValues,
  PromiseReject,
  RangeError,
  ReferenceError,
  SafeMap,
  SafeSet,
  SetPrototypeAdd,
  SetPrototypeHas,
  JSONStringify,
  ObjectDefineProperty,
  ObjectFreeze,
  ObjectPrototypeIsPrototypeOf,
  ObjectSetPrototypeOf,
  Promise,
  queueMicrotask,
  String,
  StringPrototypeStartsWith,
  Symbol,
  SymbolFor,
  SymbolIterator,
  SymbolToStringTag,
  SyntaxError,
  TypeError,
  URIError,
} = primordials;

const EMPTY_OPTIONS = ObjectFreeze(ObjectSetPrototypeOf({}, null));

const webidl = core.loadExtScript("ext:ende_webidl/00_webidl.js");
const { createFilteredInspectProxy } = core.loadExtScript(
  "ext:ende_web/01_console.js",
);
const { URL } = core.loadExtScript("ext:ende_web/00_url.js");
const { getLocationHref } = core.loadExtScript("ext:ende_web/12_location.js");
const { serializePermissions } = core.loadExtScript(
  "ext:runtime/10_permissions.js",
);
const { log } = core.loadExtScript("ext:runtime/06_util.js");
const {
  defineEventHandler,
  ErrorEvent,
  EventTarget,
  MessageEvent,
  setIsTrusted,
} = core.loadExtScript("ext:ende_web/02_event.js");
const {
  deserializeJsMessageData,
  MessagePortPrototype,
  serializeJsMessageData,
  serializeMessageData,
} = core.loadExtScript("ext:ende_web/13_message_port.js");
const { DOMException } = core.loadExtScript(
  "ext:ende_web/01_dom_exception.js",
);

function createWorker(
  specifier,
  hasSourceCode,
  sourceCode,
  permissions,
  name,
  workerType,
  closeOnIdle,
) {
  return op_create_worker({
    hasSourceCode,
    name,
    permissions: serializePermissions(permissions),
    sourceCode,
    specifier,
    workerType,
    closeOnIdle,
  });
}

function hostTerminateWorker(id) {
  op_host_terminate_worker(id);
}

function hostPostMessage(id, data) {
  op_host_post_message(id, data);
}

function hostRecvCtrl(id) {
  return op_host_recv_ctrl(id);
}

function hostRecvMessage(id) {
  return op_host_recv_message(id);
}

const privateWorkerRef = Symbol();
// Constructs a `Worker` for a worker that already exists (an extension's),
// instead of creating one from a specifier.
const existingWorkerKey = Symbol("existingWorker");

let warnedDenoOption = false;

class Worker extends EventTarget {
  #id = 0;
  #name = "";
  // For an extension's or a sandbox's worker: called once it closes.
  #onClose = undefined;
  #refCount = 1;
  #messagePromise = undefined;
  #controlPromise = undefined;

  // "RUNNING" | "CLOSED" | "TERMINATED"
  // "TERMINATED" means that any controls or messages received will be
  // discarded. "CLOSED" means that we have received a control
  // indicating that the worker is no longer running, but there might
  // still be messages left to receive.
  #status = "RUNNING";

  constructor(specifier, options = EMPTY_OPTIONS) {
    super();
    if (specifier === existingWorkerKey) {
      this.#id = options.id;
      this.#name = options.name;
      this.#onClose = options.onClose;
      this.#pollControl();
      this.#pollMessages();
      return;
    }
    specifier = String(specifier);
    const {
      ende,
      deno,
      name,
      type = "classic",
    } = options;

    if (deno !== undefined && !warnedDenoOption) {
      warnedDenoOption = true;
      globalThis.console.warn(
        'Warning: The Worker option "deno" is deprecated. Use "ende" instead.',
      );
    }

    if (options.env !== undefined || options.workerData !== undefined) {
      const unsupported = [];
      if (options.env !== undefined) unsupported[unsupported.length] = "env";
      if (options.workerData !== undefined) {
        unsupported[unsupported.length] = "workerData";
      }
      globalThis.console.warn(
        `Warning: ${
          ArrayPrototypeJoin(unsupported, ", ")
        } option(s) are not supported ` +
          "for web Workers and will be ignored.",
      );
    }

    if (type === "node") {
      throw new TypeError(
        'Worker type "node" is not supported in Ende: use type "module" instead.',
      );
    }
    const workerType = webidl.converters["WorkerType"](type);

    if (
      StringPrototypeStartsWith(specifier, "./") ||
      StringPrototypeStartsWith(specifier, "../") ||
      StringPrototypeStartsWith(specifier, "/") || workerType === "classic"
    ) {
      const baseUrl = getLocationHref();
      if (baseUrl != null) {
        specifier = new URL(specifier, baseUrl).href;
      }
    }

    this.#name = name;
    let hasSourceCode, sourceCode;
    if (workerType === "classic") {
      hasSourceCode = true;
      sourceCode = `importScripts(${JSONStringify(specifier)});`;
    } else {
      hasSourceCode = false;
      sourceCode = "";
    }

    const id = createWorker(
      specifier,
      hasSourceCode,
      sourceCode,
      (ende ?? deno)?.permissions,
      this.#name,
      workerType,
      false,
    );
    this.#id = id;
    this.#pollControl();
    this.#pollMessages();
  }

  [privateWorkerRef](ref) {
    if (ref) {
      this.#refCount++;
    } else {
      this.#refCount--;
    }

    if (!ref && this.#refCount == 0) {
      if (this.#controlPromise) {
        core.unrefOpPromise(this.#controlPromise);
      }
      if (this.#messagePromise) {
        core.unrefOpPromise(this.#messagePromise);
      }
    } else if (ref && this.#refCount == 1) {
      if (this.#controlPromise) {
        core.refOpPromise(this.#controlPromise);
      }
      if (this.#messagePromise) {
        core.refOpPromise(this.#messagePromise);
      }
    }
  }

  #handleError(e) {
    const event = new ErrorEvent("error", {
      cancelable: true,
      message: e.message,
      lineno: e.lineNumber ? e.lineNumber : undefined,
      colno: e.columnNumber ? e.columnNumber : undefined,
      filename: e.fileName,
      error: null,
    });

    this.dispatchEvent(event);
    // Don't bubble error event to window for loader errors (`!e.fileName`).
    // TODO(nayeemrmn): It's not correct to use `e.fileName` to detect user
    // errors. It won't be there for non-awaited async ops for example.
    if (e.fileName && !event.defaultPrevented) {
      globalThis.dispatchEvent(event);
    }

    return event.defaultPrevented;
  }

  #pollControl = async () => {
    while (this.#status === "RUNNING") {
      this.#controlPromise = hostRecvCtrl(this.#id);
      if (this.#refCount < 1) {
        core.unrefOpPromise(this.#controlPromise);
      }
      const { 0: type, 1: data } = await this.#controlPromise;

      // If terminate was called then we ignore all messages
      if (this.#status === "TERMINATED") {
        return;
      }

      switch (type) {
        case 1: { // TerminalError
          this.#status = "CLOSED";
        } /* falls through */
        case 2: { // Error
          if (!this.#handleError(data)) {
            throw new Error("Unhandled error in child worker.");
          }
          break;
        }
        case 3: { // Close
          log(`Host got "close" message from worker: ${this.#name}`);
          this.#status = "CLOSED";
          this.#onClose?.();
          return;
        }
        default: {
          throw new Error(`Unknown worker event: "${type}"`);
        }
      }
    }
  };

  #dispatchWorkerMessage(data) {
    let message, transferables;
    try {
      const v = deserializeJsMessageData(data);
      message = v[0];
      transferables = v[1];
    } catch (err) {
      const event = new MessageEvent("messageerror", {
        cancelable: false,
        data: err,
      });
      setIsTrusted(event, true);
      this.dispatchEvent(event);
      return false;
    }
    const event = new MessageEvent("message", {
      cancelable: false,
      data: message,
      // Skip the transferables filter for the common no-transferables case.
      // Passing `undefined` lets the MessageEvent constructor take its cheap
      // `ports == null` branch (a single frozen empty array, no iterator
      // validation) instead of allocating a filtered array per message.
      ports: transferables.length === 0 ? undefined : ArrayPrototypeFilter(
        transferables,
        (t) => ObjectPrototypeIsPrototypeOf(MessagePortPrototype, t),
      ),
    });
    setIsTrusted(event, true);
    this.dispatchEvent(event);
    return true;
  }

  #pollMessages = async () => {
    while (this.#status !== "TERMINATED") {
      this.#messagePromise = hostRecvMessage(this.#id);
      if (this.#refCount < 1) {
        core.unrefOpPromise(this.#messagePromise);
      }
      const data = await this.#messagePromise;
      if (this.#status === "TERMINATED" || data === null) {
        return;
      }
      if (!this.#dispatchWorkerMessage(data)) return;
      // Drain messages already queued on the host side instead of taking the
      // async op + Promise path for each, mirroring the worker global
      // (99_main.js) receive loop. The whole burst is
      // processed within this event-loop turn; the batch limit prevents
      // starving the event loop under a sustained flood.
      for (let i = 0; i < 1000 && this.#status !== "TERMINATED"; i++) {
        const syncData = op_host_recv_message_sync(this.#id);
        if (syncData === null) break;
        // Each message dispatch is its own task. Yield a microtask before
        // delivering this already-dequeued message so a handler that re-armed
        // itself in a microtask after the previous dispatch (e.g. reassigning
        // `onmessage` inside a `.then`, as WPT
        // workers/Worker-structure-message.html does) is installed first --
        // otherwise the message reaches the stale handler and is lost. A
        // synchronous checkpoint can't help here: V8 won't run microtasks
        // reentrantly while we are already inside one.
        await new Promise((resolve) => queueMicrotask(() => resolve()));
        if (this.#status === "TERMINATED") return;
        if (!this.#dispatchWorkerMessage(syncData)) return;
      }
    }
  };

  postMessage(message, transferOrOptions = undefined) {
    const prefix = "Failed to execute 'postMessage' on 'MessagePort'";
    webidl.requiredArguments(arguments.length, 1, prefix);
    if (this.#status !== "RUNNING") return;
    // Fast path: no transferables
    if (
      transferOrOptions === undefined ||
      transferOrOptions === null ||
      (arguments.length <= 1)
    ) {
      op_host_post_message_raw(
        this.#id,
        serializeMessageData(message, (err) => {
          throw new DOMException(err, "DataCloneError");
        }),
      );
      return;
    }
    message = webidl.converters.any(message);
    let options;
    if (
      webidl.type(transferOrOptions) === "Object" &&
      transferOrOptions !== undefined &&
      transferOrOptions[SymbolIterator] !== undefined
    ) {
      const transfer = webidl.converters["sequence<object>"](
        transferOrOptions,
        prefix,
        "Argument 2",
      );
      options = { transfer };
    } else {
      options = webidl.converters.StructuredSerializeOptions(
        transferOrOptions,
        prefix,
        "Argument 2",
      );
    }
    const { transfer } = options;
    const data = serializeJsMessageData(message, transfer);
    hostPostMessage(this.#id, data);
  }

  terminate() {
    if (this.#status !== "TERMINATED") {
      this.#status = "TERMINATED";
      hostTerminateWorker(this.#id);
    }
  }

  [SymbolFor("Deno.privateCustomInspect")](inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(WorkerPrototype, this),
        keys: [
          "onerror",
          "onmessage",
          "onmessageerror",
        ],
      }),
      inspectOptions,
    );
  }
}

const WorkerPrototype = Worker.prototype;
ObjectDefineProperty(WorkerPrototype, SymbolToStringTag, {
  __proto__: null,
  value: "Worker",
  writable: false,
  enumerable: false,
  configurable: true,
});

defineEventHandler(Worker.prototype, "error");
defineEventHandler(Worker.prototype, "message");
defineEventHandler(Worker.prototype, "messageerror");

webidl.converters["WorkerType"] = webidl.createEnumConverter("WorkerType", [
  "classic",
  "module",
]);

// Sandboxed extensions (`extension:@scope/name`). The module an app imports
// is a generated proxy that calls `start` once and `call` per export; the
// extension itself runs in its own worker with its own permissions, started
// by Rust from the app's configuration alone (op_create_extension_worker).
// Only values that can be structured-cloned cross, plus `Ende.Root`s, which
// are transferred (see ext/fs/30_fs.js).

/** name -> { worker, ready, pending, nextId, failure } */
const extensionWorkers = new SafeMap();

function startExtension(name) {
  let extension = MapPrototypeGet(extensionWorkers, name);
  if (extension === undefined) {
    const id = op_create_extension_worker(name);
    const worker = new Worker(existingWorkerKey, {
      id,
      name: `extension:${name}`,
      onClose: () => failExtension(current, stoppedError(`extension:${name}`)),
    });
    extension = {
      worker,
      ready: undefined,
      resolveReady: undefined,
      rejectReady: undefined,
      pending: new SafeMap(),
      nextId: 0,
      failure: undefined,
    };
    const current = extension;
    current.ready = new Promise((resolve, reject) => {
      current.resolveReady = resolve;
      current.rejectReady = reject;
    });
    worker.onmessage = (event) => onExtensionMessage(current, event.data);
    worker.onerror = (event) => {
      event.preventDefault();
      failExtension(current, new Error(event.message));
    };
    MapPrototypeSet(extensionWorkers, name, extension);
  }
  return extension.ready;
}

// A worker that closed (`Ende.exit()`, `close()`) fails the calls in flight
// and later ones instead of leaving them pending forever.
function stoppedError(label) {
  return new Error(
    `${label} stopped: its worker exited or failed, so calls into it fail`,
  );
}

function failExtension(extension, error) {
  if (extension.failure !== undefined) return;
  extension.failure = error;
  extension.rejectReady(error);
  const pending = extension.pending;
  extension.pending = new SafeMap();
  MapPrototypeForEach(pending, (call) => call.reject(error));
}

function onExtensionMessage(extension, data) {
  if (data === null || typeof data !== "object") return;
  if (data.ready === true) {
    // An idle extension doesn't keep the process alive; a call in flight
    // does (see callExtension).
    extension.worker[privateWorkerRef](false);
    extension.resolveReady();
    return;
  }
  const call = MapPrototypeGet(extension.pending, data.id);
  if (call === undefined) return;
  MapPrototypeDelete(extension.pending, data.id);
  if (extension.pending.size === 0) {
    extension.worker[privateWorkerRef](false);
  }
  if (data.ok === true) {
    call.resolve(data.value);
  } else if (data.error !== undefined) {
    call.reject(reviveExtensionError(data.error));
  } else {
    call.reject(data.thrown);
  }
}

function callExtension(name, exportName, args) {
  const extension = MapPrototypeGet(extensionWorkers, name);
  if (extension === undefined) {
    return PromiseReject(
      new Error(`Extension "${name}" hasn't been started`),
    );
  }
  if (extension.failure !== undefined) {
    return PromiseReject(extension.failure);
  }
  const id = extension.nextId++;
  try {
    extension.worker.postMessage(
      { id, name: exportName, args },
      transferredRoots(args),
    );
  } catch (error) {
    return PromiseReject(error);
  }
  if (extension.pending.size === 0) {
    extension.worker[privateWorkerRef](true);
  }
  return new Promise((resolve, reject) => {
    MapPrototypeSet(extension.pending, id, { resolve, reject });
  });
}

/** The `Ende.Root`s inside `value`, which are transferred rather than cloned. */
function transferredRoots(value, roots = [], seen = new SafeSet()) {
  if (value === null || typeof value !== "object") return roots;
  if (SetPrototypeHas(seen, value)) return roots;
  SetPrototypeAdd(seen, value);
  if (value[core.hostObjectBrand] === "EndeRoot") {
    ArrayPrototypePush(roots, value);
    return roots;
  }
  if (ArrayIsArray(value)) {
    for (let i = 0; i < value.length; i++) {
      transferredRoots(value[i], roots, seen);
    }
  } else if (ObjectGetPrototypeOf(value) === ObjectPrototype) {
    const values = ObjectValues(value);
    for (let i = 0; i < values.length; i++) {
      transferredRoots(values[i], roots, seen);
    }
  }
  return roots;
}

// An error thrown in an extension, sent as `{ name, message, stack,
// isDOMException }`: the structured clone of an error keeps only the
// built-in error types, not `Ende.errors.*` or `DOMException`.
const builtinErrors = {
  __proto__: null,
  Error,
  EvalError,
  RangeError,
  ReferenceError,
  SyntaxError,
  TypeError,
  URIError,
};

// `name` and `message` come from the extension, so they're data: only an
// own property of the error tables picks a class (never `constructor` or
// `hasOwnProperty`), and anything that fails to construct becomes a plain
// `Error` carrying the same name and message.
function reviveExtensionError({ name, message, stack, isDOMException }) {
  const { errors } = core.loadExtScript("ext:runtime/01_errors.js");
  let error;
  try {
    if (isDOMException === true) {
      error = new DOMException(message, name);
    } else if (
      typeof name === "string" && ObjectHasOwn(errors, name) &&
      typeof errors[name] === "function"
    ) {
      error = new errors[name](message);
    } else if (
      typeof name === "string" && ObjectHasOwn(builtinErrors, name)
    ) {
      error = new builtinErrors[name](message);
    }
  } catch {
    error = undefined;
  }
  if (error === undefined) {
    error = new Error(String(message));
    ObjectDefineProperty(error, "name", {
      __proto__: null,
      value: String(name),
      writable: true,
      configurable: true,
    });
  }
  ObjectDefineProperty(error, "stack", {
    __proto__: null,
    value: stack,
    writable: true,
    configurable: true,
  });
  return error;
}

// Sandboxed packages (`isolate` in ende.json). An import of a sandboxed
// module from outside its sandbox gets a generated proxy that calls `start`
// once; the sandbox itself runs in a worker of its own, started by Rust from
// the app's configuration alone (op_create_sandbox_worker), whose
// permissions are the rule's grant checked together with the app's own. One
// worker serves every entry of a sandbox. As with extensions, only
// structured-cloned values and transferred `Ende.Root`s cross.

/** rule -> { worker, ready, pending, nextId, failure, entries } */
const sandboxWorkers = new SafeMap();

function sandboxWorker(rule) {
  let sandbox = MapPrototypeGet(sandboxWorkers, rule);
  if (sandbox === undefined) {
    const id = op_create_sandbox_worker(rule);
    const worker = new Worker(existingWorkerKey, {
      id,
      name: `isolate["${rule}"]`,
      onClose: () => failExtension(current, stoppedError(`isolate["${rule}"]`)),
    });
    sandbox = {
      worker,
      ready: undefined,
      resolveReady: undefined,
      rejectReady: undefined,
      pending: new SafeMap(),
      nextId: 0,
      failure: undefined,
      /** entry -> Promise<SafeMap<name, { kind, value, what }>> */
      entries: new SafeMap(),
    };
    const current = sandbox;
    current.ready = new Promise((resolve, reject) => {
      current.resolveReady = resolve;
      current.rejectReady = reject;
    });
    worker.onmessage = (event) => onExtensionMessage(current, event.data);
    worker.onerror = (event) => {
      event.preventDefault();
      failExtension(
        current,
        new Error(
          `${stoppedError(`isolate["${rule}"]`).message}: ${event.message}`,
        ),
      );
    };
    MapPrototypeSet(sandboxWorkers, rule, sandbox);
  }
  return sandbox;
}

function sandboxRequest(sandbox, message, transfer) {
  if (sandbox.failure !== undefined) {
    return PromiseReject(sandbox.failure);
  }
  const id = sandbox.nextId++;
  message.id = id;
  try {
    sandbox.worker.postMessage(message, transfer);
  } catch (error) {
    return PromiseReject(error);
  }
  if (sandbox.pending.size === 0) {
    sandbox.worker[privateWorkerRef](true);
  }
  return new Promise((resolve, reject) => {
    MapPrototypeSet(sandbox.pending, id, { resolve, reject });
  });
}

/** The exports of `entry`, loaded in the sandbox once per entry. */
function sandboxEntry(sandbox, entry) {
  let exports = MapPrototypeGet(sandbox.entries, entry);
  if (exports === undefined) {
    exports = (async () => {
      await sandbox.ready;
      const list = await sandboxRequest(sandbox, { load: entry }, []);
      const map = new SafeMap();
      for (let i = 0; i < list.length; i++) {
        const { 0: name, 1: kind, 2: payload } = list[i];
        MapPrototypeSet(map, name, {
          kind,
          value: kind === "value" ? payload : undefined,
          what: kind === "unsupported" ? String(payload) : undefined,
          binding: undefined,
        });
      }
      return map;
    })();
    MapPrototypeSet(sandbox.entries, entry, exports);
  }
  return exports;
}

function sandboxBinding(sandbox, entry, name, info) {
  if (info.kind === "value") return info.value;
  if (info.binding === undefined) {
    info.binding = (...args) =>
      sandboxRequest(
        sandbox,
        { entry, name, args },
        transferredRoots(args),
      );
  }
  return info.binding;
}

function unsupportedExport(rule, entry, name, what) {
  return new TypeError(
    `Cannot import "${name}" from "${entry}": it is ${what}, which can't leave the sandbox of isolate["${rule}"]. A sandboxed package can only export functions (called async, with arguments and results structured-cloned) and structured-clonable constants`,
  );
}

// Starts the sandbox of `rule` (once per isolate), loads `entry` in it (once
// per entry), and returns the bindings of `named` then `implicit`, in that
// order. `named` are the exports the importer names; `implicit` are the
// others a namespace import sees. Either way an export the sandbox can't
// hand over is an error at the import, naming it.
async function startSandbox(rule, entry, named, implicit) {
  const sandbox = sandboxWorker(rule);
  const exports = await sandboxEntry(sandbox, entry);
  const bindings = [];
  for (let i = 0; i < named.length; i++) {
    const name = named[i];
    const info = MapPrototypeGet(exports, name);
    if (info === undefined) {
      throw new SyntaxError(
        `The requested module "${entry}" does not provide an export named "${name}"`,
      );
    }
    if (info.kind === "unsupported") {
      throw unsupportedExport(rule, entry, name, info.what);
    }
    ArrayPrototypePush(
      bindings,
      sandboxBinding(sandbox, entry, name, info),
    );
  }
  for (let i = 0; i < implicit.length; i++) {
    const name = implicit[i];
    const info = MapPrototypeGet(exports, name);
    if (info === undefined || info.kind === "unsupported") {
      throw new TypeError(
        `${
          unsupportedExport(rule, entry, name, info?.what ?? "missing").message
        }. A namespace import leaves out the exports that can't cross only when their kind shows in the source; this one's shows only when the sandbox runs, so import the exports you use by name`,
      );
    }
    ArrayPrototypePush(
      bindings,
      sandboxBinding(sandbox, entry, name, info),
    );
  }
  return bindings;
}

return {
  Worker,
  extensions: { start: startExtension, call: callExtension },
  sandboxes: { start: startSandbox },
};
})();
