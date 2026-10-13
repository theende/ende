// Copyright 2018-2026 the Deno authors. MIT license.
import * as async from "checkin:async";
import * as console from "checkin:console";
import * as error from "checkin:error";
import * as timers from "checkin:timers";
import * as worker from "checkin:worker";
import * as throw_ from "checkin:throw";
import * as object from "checkin:object";
import * as callsite from "checkin:callsite";
async;
error;
throw_;
object;
callsite;

globalThis.console = console.console;
globalThis.setTimeout = timers.setTimeout;
globalThis.setInterval = timers.setInterval;
globalThis.clearTimeout = timers.clearTimeout;
globalThis.clearInterval = timers.clearInterval;
globalThis.setImmediate = timers.setImmediate;
globalThis.clearImmediate = timers.clearImmediate;
globalThis.Worker = worker.Worker;
Ende.core.addMainModuleHandler((module) => {
  if (onMainModuleCb) onMainModuleCb(module);
});
let onMainModuleCb = () => {};
Reflect.defineProperty(globalThis, "onmainmodule", {
  set: (cb) => {
    onMainModuleCb = cb;
  },
});
Reflect.defineProperty(globalThis, "onerror", {
  set: (cb) => {
    if (cb) {
      Ende.core.setReportExceptionCallback((error) => {
        let defaultPrevented = false;
        cb({
          error,
          preventDefault: () => defaultPrevented = true,
        });
        if (!defaultPrevented) {
          Ende.core.reportUnhandledException(error);
        }
      });
    } else {
      Ende.core.setReportExceptionCallback(null);
    }
  },
});
Reflect.defineProperty(globalThis, "onunhandledrejection", {
  set: (cb) => {
    if (cb) {
      Ende.core.setUnhandledPromiseRejectionHandler((promise, reason) => {
        let defaultPrevented = false;
        cb({
          promise,
          reason,
          preventDefault: () => defaultPrevented = true,
        });
        return defaultPrevented;
      });
    } else {
      Ende.core.setUnhandledPromiseRejectionHandler(null);
    }
  },
});
Reflect.defineProperty(globalThis, "onrejectionhandled", {
  set: (cb) => {
    if (cb) {
      Ende.core.setHandledPromiseRejectionHandler((promise, reason) => {
        cb({
          promise,
          reason,
        });
      });
    } else {
      Ende.core.setHandledPromiseRejectionHandler(null);
    }
  },
});
Ende.unrefTimer = timers.unrefTimer;
Ende.refTimer = timers.refTimer;
Ende.unrefImmediate = timers.unrefImmediate;
