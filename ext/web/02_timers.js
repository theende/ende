// Copyright 2018-2026 the Deno authors. MIT license.

// Web timers (setTimeout/setInterval) built directly on core.createTimer.
// Adds WHATWG-specific behavior on top:
// - webidl type coercion
// - string callback eval (WHATWG spec)
// - timer nesting depth tracking and 4ms clamping (WHATWG spec)
// - numeric timer IDs
// - AsyncContext propagation across the callback boundary

(function () {
const { core, primordials } = __bootstrap;
const { op_defer } = core.ops;
const {
  createTimer,
  cancelTimer,
  refTimer: coreRefTimer,
  unrefTimer: coreUnrefTimer,
  getAsyncContext,
  setAsyncContext,
} = core;
const {
  MapPrototypeDelete,
  MapPrototypeGet,
  MapPrototypeSet,
  PromisePrototypeThen,
  ReflectApply,
  SafeMap,
  TypeError,
  indirectEval,
} = primordials;

const webidl = core.loadExtScript("ext:ende_webidl/00_webidl.js");

// Map numeric timer IDs to internal core timer objects so clearTimeout /
// clearInterval / refTimer / unrefTimer can look them up by id.
const activeTimers = new SafeMap();

// WHATWG timer nesting depth tracking.
let timerDepth = 0;

// WHATWG timer initialization steps: once the nesting level of the task that
// schedules a timer is greater than 5, timeouts below 4ms are raised to 4ms.
function clampTimeout(timeout, nestingLevel) {
  return nestingLevel > 5 && timeout < 4 ? 4 : timeout;
}

function checkThis(thisArg) {
  if (thisArg !== null && thisArg !== undefined && thisArg !== globalThis) {
    throw new TypeError("Illegal invocation");
  }
}

/**
 * Call a callback function after a delay.
 */
function setTimeout(callback, timeout = 0, ...args) {
  checkThis(this);
  if (typeof callback !== "function") {
    const unboundCallback = webidl.converters.DOMString(callback);
    callback = () => indirectEval(unboundCallback);
  }
  const unboundCallback = callback;
  const asyncContext = getAsyncContext();
  const depth = timerDepth;
  let id = 0;
  const wrappedCallback = function () {
    const oldContext = getAsyncContext();
    const prevDepth = timerDepth;
    try {
      setAsyncContext(asyncContext);
      timerDepth = depth + 1;
      ReflectApply(unboundCallback, globalThis, args);
    } finally {
      timerDepth = prevDepth;
      setAsyncContext(oldContext);
      MapPrototypeDelete(activeTimers, id);
    }
  };
  timeout = clampTimeout(webidl.converters.long(timeout), depth);
  const timer = createTimer(wrappedCallback, timeout, undefined, false, true);
  id = timer._timerId;
  MapPrototypeSet(activeTimers, id, timer);
  return id;
}

/**
 * Call a callback function repeatedly at a given interval.
 */
function setInterval(callback, timeout = 0, ...args) {
  checkThis(this);
  if (typeof callback !== "function") {
    const unboundCallback = webidl.converters.DOMString(callback);
    callback = () => indirectEval(unboundCallback);
  }
  const unboundCallback = callback;
  const asyncContext = getAsyncContext();
  // Nesting level of this interval's task; it grows with every repetition,
  // so short intervals are clamped to 4ms after the fifth one.
  let depth = timerDepth + 1;
  const wrappedCallback = function () {
    const oldContext = getAsyncContext();
    const prevDepth = timerDepth;
    try {
      setAsyncContext(asyncContext);
      timerDepth = depth;
      ReflectApply(unboundCallback, globalThis, args);
    } finally {
      timerDepth = prevDepth;
      setAsyncContext(oldContext);
      timer._repeat = clampTimeout(timer._repeat, depth);
      depth++;
    }
  };
  timeout = clampTimeout(webidl.converters.long(timeout), depth - 1);
  const timer = createTimer(wrappedCallback, timeout, undefined, true, true);
  const id = timer._timerId;
  MapPrototypeSet(activeTimers, id, timer);
  return id;
}

/**
 * Clear a timeout or interval.
 */
function clearTimeout(id = 0) {
  checkThis(this);
  id = webidl.converters.long(id);
  const timer = MapPrototypeGet(activeTimers, id);
  if (timer) {
    cancelTimer(timer);
    MapPrototypeDelete(activeTimers, id);
  }
}

/**
 * Clear a timeout or interval.
 */
function clearInterval(id = 0) {
  checkThis(this);
  id = webidl.converters.long(id);
  const timer = MapPrototypeGet(activeTimers, id);
  if (timer) {
    cancelTimer(timer);
    MapPrototypeDelete(activeTimers, id);
  }
}

/**
 * Mark a timer as not blocking event loop exit.
 */
function unrefTimer(id) {
  const timer = MapPrototypeGet(activeTimers, id);
  if (timer) {
    coreUnrefTimer(timer);
  }
}

/**
 * Mark a timer as blocking event loop exit.
 */
function refTimer(id) {
  const timer = MapPrototypeGet(activeTimers, id);
  if (timer) {
    coreRefTimer(timer);
  }
}

// Defer to avoid starving the event loop. Not using queueMicrotask()
// for that reason: it lets promises make forward progress but can
// still starve other parts of the event loop.
function defer(go) {
  PromisePrototypeThen(op_defer(), () => go());
}

return {
  clearInterval,
  clearTimeout,
  defer,
  refTimer,
  setInterval,
  setTimeout,
  unrefTimer,
};
})();
