// Copyright 2018-2026 the Deno authors. MIT license.

// @ts-check
/// <reference path="../../core/internal.d.ts" />

(function () {
const { core, primordials } = __bootstrap;
const {
  ObjectDefineProperty,
  SafeArrayIterator,
  Symbol,
  SymbolToStringTag,
  TypeError,
} = primordials;
const { EventTarget } = core.loadExtScript("ext:ende_web/02_event.js");

const illegalConstructorKey = Symbol("illegalConstructorKey");

class Window extends EventTarget {
  constructor(key = null) {
    if (key !== illegalConstructorKey) {
      throw new TypeError("Illegal constructor");
    }
    super();
  }
}

class WorkerGlobalScope extends EventTarget {
  constructor(key = null) {
    if (key != illegalConstructorKey) {
      throw new TypeError("Illegal constructor");
    }
    super();
  }
}

class DedicatedWorkerGlobalScope extends WorkerGlobalScope {
  constructor(key = null) {
    if (key != illegalConstructorKey) {
      throw new TypeError("Illegal constructor");
    }
    super();
  }
}

// Web IDL makes the toStringTag of an interface prototype a data property.
for (
  const iface of new SafeArrayIterator([
    Window,
    WorkerGlobalScope,
    DedicatedWorkerGlobalScope,
  ])
) {
  ObjectDefineProperty(iface.prototype, SymbolToStringTag, {
    __proto__: null,
    value: iface.name,
    writable: false,
    enumerable: false,
    configurable: true,
  });
}

const dedicatedWorkerGlobalScopeConstructorDescriptor = {
  __proto__: null,
  configurable: true,
  enumerable: false,
  value: DedicatedWorkerGlobalScope,
  writable: true,
};

const windowConstructorDescriptor = {
  __proto__: null,
  configurable: true,
  enumerable: false,
  value: Window,
  writable: true,
};

const workerGlobalScopeConstructorDescriptor = {
  __proto__: null,
  configurable: true,
  enumerable: false,
  value: WorkerGlobalScope,
  writable: true,
};

return {
  DedicatedWorkerGlobalScope,
  dedicatedWorkerGlobalScopeConstructorDescriptor,
  Window,
  windowConstructorDescriptor,
  WorkerGlobalScope,
  workerGlobalScopeConstructorDescriptor,
};
})();
