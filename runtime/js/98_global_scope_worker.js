// Copyright 2018-2026 the Deno authors. MIT license.

import { core, primordials } from "ext:core/mod.js";
import {
  op_bootstrap_language,
  op_bootstrap_numcpus,
  op_bootstrap_user_agent,
} from "ext:core/ops";
import {
  assertGlobalReceiver,
  selfDescriptor,
} from "ext:runtime/98_global_scope_shared.js";
const {
  ObjectDefineProperties,
  ObjectDefineProperty,
  ObjectPrototypeIsPrototypeOf,
  StringPrototypeSlice,
  StringPrototypeToUpperCase,
  SymbolFor,
  SymbolToStringTag,
} = primordials;

const location = core.loadExtScript("ext:ende_web/12_location.js");
const console = core.loadExtScript("ext:ende_web/01_console.js");
const webidl = core.loadExtScript("ext:ende_webidl/00_webidl.js");
const globalInterfaces = core.loadExtScript(
  "ext:ende_web/04_global_interfaces.js",
);
const loadLocks = core.createLazyLoader("ext:ende_web/locks.js");
const { loadWebGPU } = core.loadExtScript("ext:ende_webgpu/00_init.js");
import {
  NavigatorUAData,
  navigatorUAData,
} from "ext:runtime/97_navigator_user_agent_data.js";

/**
 * @param {string} arch
 * @param {string} platform
 * @returns {string}
 */
function getNavigatorPlatform(arch, platform) {
  switch (platform) {
    case "darwin":
      // On macOS, modern browsers return 'MacIntel' even if running on Apple Silicon.
      return "MacIntel";

    case "windows":
      // On Windows, modern browsers return 'Win32' even if running on a 64-bit version of Windows.
      // https://developer.mozilla.org/en-US/docs/Web/API/Navigator/platform#usage_notes
      return "Win32";

    case "linux":
      return `Linux ${arch}`;

    case "freebsd":
      if (arch === "x86_64") {
        return "FreeBSD amd64";
      }
      return `FreeBSD ${arch}`;

    case "solaris":
      return `SunOS ${arch}`;

    case "aix":
      return "AIX";

    default:
      return `${StringPrototypeToUpperCase(platform[0])}${
        StringPrototypeSlice(platform, 1)
      } ${arch}`;
  }
}

function memoizeLazy(f) {
  let v_ = null;
  return () => {
    if (v_ === null) {
      v_ = f();
    }
    return v_;
  };
}

const numCpus = memoizeLazy(() => op_bootstrap_numcpus());
const userAgent = memoizeLazy(() => op_bootstrap_user_agent());
const language = memoizeLazy(() => op_bootstrap_language());
const platform = memoizeLazy(() =>
  getNavigatorPlatform(core.build.arch, core.build.os)
);

class WorkerNavigator {
  constructor() {
    webidl.illegalConstructor();
  }

  [SymbolFor("Deno.privateCustomInspect")](inspect, inspectOptions) {
    return inspect(
      console.createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(WorkerNavigatorPrototype, this),
        keys: [
          "hardwareConcurrency",
          "userAgent",
          "language",
          "languages",
          "platform",
          "userAgentData",
        ],
      }),
      inspectOptions,
    );
  }
}

const workerNavigator = webidl.createBranded(WorkerNavigator);

ObjectDefineProperties(WorkerNavigator.prototype, {
  gpu: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      const webgpu = loadWebGPU();
      webgpu.initGPU();
      return webgpu.gpu;
    },
  },
  hardwareConcurrency: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return numCpus();
    },
  },
  userAgent: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return userAgent();
    },
  },
  language: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return language();
    },
  },
  languages: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return [language()];
    },
  },
  locks: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return loadLocks().locks;
    },
  },
  platform: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return platform();
    },
  },
  userAgentData: {
    __proto__: null,
    configurable: true,
    enumerable: true,
    get() {
      webidl.assertBranded(this, WorkerNavigatorPrototype);
      return navigatorUAData;
    },
  },
});
const WorkerNavigatorPrototype = WorkerNavigator.prototype;
ObjectDefineProperty(WorkerNavigatorPrototype, SymbolToStringTag, {
  __proto__: null,
  value: "WorkerNavigator",
  writable: false,
  enumerable: false,
  configurable: true,
});

const workerRuntimeGlobalProperties = {
  WorkerLocation: location.workerLocationConstructorDescriptor,
  location: location.workerLocationDescriptor,
  WorkerGlobalScope: globalInterfaces.workerGlobalScopeConstructorDescriptor,
  DedicatedWorkerGlobalScope:
    globalInterfaces.dedicatedWorkerGlobalScopeConstructorDescriptor,
  WorkerNavigator: core.propNonEnumerable(WorkerNavigator),
  navigator: core.propGetterOnly(function () {
    assertGlobalReceiver(this);
    return workerNavigator;
  }),
  NavigatorUAData: core.propNonEnumerable(NavigatorUAData),
  self: selfDescriptor,
};

// Not [Serializable]: structuredClone throws a DataCloneError.
{
  const { markNotSerializable } = core.loadExtScript(
    "ext:ende_web/13_message_port.js",
  );
  markNotSerializable(WorkerNavigator.prototype);
}

export { workerRuntimeGlobalProperties };
