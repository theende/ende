// Copyright 2018-2026 the Deno authors. MIT license.
import * as module from "checkin:testing";
import { assert, test } from "checkin:testing";

test(function testIsAnyArrayBuffer() {
  assert(Ende.core.isAnyArrayBuffer(new ArrayBuffer(4)));
  assert(Ende.core.isAnyArrayBuffer(new SharedArrayBuffer(4)));
  assert(!Ende.core.isAnyArrayBuffer(new Uint8Array(4)));
});

test(function testIsArgumentsObject() {
  let args: IArguments;
  (function () {
  args = arguments;
  })();
  assert(Ende.core.isArgumentsObject(args));
  assert(!Ende.core.isArgumentsObject({}));
});

test(function testIsArrayBuffer() {
  assert(Ende.core.isArrayBuffer(new ArrayBuffer(4)));
  assert(!Ende.core.isArrayBuffer(new SharedArrayBuffer(4)));
  assert(!Ende.core.isArrayBuffer(new Uint8Array(4)));
});

test(function testIsArrayBufferView() {
  assert(Ende.core.isArrayBufferView(new DataView(new ArrayBuffer(4))));
  assert(Ende.core.isArrayBufferView(new Uint8Array(4)));
  assert(!Ende.core.isArrayBufferView(new ArrayBuffer(4)));
});

test(function testIsAsyncFunction() {
  assert(Ende.core.isAsyncFunction(async function () {}));
  assert(Ende.core.isAsyncFunction(async function* () {}));
  assert(!Ende.core.isAsyncFunction(function () {}));
  assert(!Ende.core.isAsyncFunction(function* () {}));
});

test(function testIsBigIntObject() {
  assert(Ende.core.isBigIntObject(Object(1n)));
  assert(!Ende.core.isBigIntObject(1n));
  assert(!Ende.core.isBigIntObject(1));
});

test(function testIsBooleanObject() {
  assert(Ende.core.isBooleanObject(new Boolean(true)));
  assert(!Ende.core.isBooleanObject(true));
  assert(!Ende.core.isBooleanObject("true"));
});

test(function testIsBoxedPrimitive() {
  assert(Ende.core.isBoxedPrimitive(Object(1n)));
  assert(Ende.core.isBoxedPrimitive(new Boolean(true)));
  assert(Ende.core.isBoxedPrimitive(new Number(1)));
  assert(Ende.core.isBoxedPrimitive(Object(Symbol())));
  assert(Ende.core.isBoxedPrimitive(new String("str")));
  assert(!Ende.core.isBoxedPrimitive(1n));
  assert(!Ende.core.isBoxedPrimitive(true));
  assert(!Ende.core.isBoxedPrimitive(1));
  assert(!Ende.core.isBoxedPrimitive(Symbol()));
  assert(!Ende.core.isBoxedPrimitive("str"));
});

test(function testIsDataView() {
  assert(Ende.core.isDataView(new DataView(new ArrayBuffer(4))));
  assert(!Ende.core.isDataView(new Uint8Array(4)));
  assert(!Ende.core.isDataView(new ArrayBuffer(4)));
});

test(function testIsDate() {
  assert(Ende.core.isDate(new Date()));
  assert(!Ende.core.isDate({}));
});

test(function testIsGeneratorFunction() {
  assert(Ende.core.isGeneratorFunction(async function* () {}));
  assert(Ende.core.isGeneratorFunction(function* () {}));
  assert(!Ende.core.isGeneratorFunction(async function () {}));
  assert(!Ende.core.isGeneratorFunction(function () {}));
});

test(function testIsGeneratorObject() {
  const generator = (function* () {})();
  assert(Ende.core.isGeneratorObject(generator));
  assert(!Ende.core.isGeneratorObject({}));
});

test(function testIsMap() {
  assert(Ende.core.isMap(new Map()));
  assert(!Ende.core.isMap(new Set()));
  assert(!Ende.core.isMap(new WeakMap()));
  assert(!Ende.core.isMap(new WeakSet()));
});

test(function testIsMapIterator() {
  const map = new Map();
  assert(Ende.core.isMapIterator(map.values()));
  assert(!Ende.core.isMapIterator(map));
});

test(function testIsModuleNamespaceObject() {
  assert(Ende.core.isModuleNamespaceObject(module));
  assert(!Ende.core.isModuleNamespaceObject({}));
});

test(function testIsNativeError() {
  assert(Ende.core.isNativeError(new Error()));
  assert(Ende.core.isNativeError(new TypeError()));
  assert(!Ende.core.isNativeError({}));
});

test(function testIsNativeError() {
  assert(Ende.core.isNumberObject(new Number(1)));
  assert(!Ende.core.isNumberObject(1));
});

test(function testIsPromise() {
  assert(Ende.core.isPromise(new Promise((resolve) => resolve(1))));
  assert(!Ende.core.isPromise({}));
});

test(function testIsProxy() {
  assert(Ende.core.isProxy(new Proxy({}, {})));
  assert(!Ende.core.isProxy({}));
});

test(function testIsRegExp() {
  assert(Ende.core.isRegExp(/foo/));
  assert(!Ende.core.isRegExp({}));
});

test(function testIsSet() {
  assert(Ende.core.isSet(new Set()));
  assert(!Ende.core.isSet(new Map()));
  assert(!Ende.core.isSet(new WeakSet()));
  assert(!Ende.core.isSet(new WeakMap()));
});

test(function testIsSetIterator() {
  const set = new Set();
  assert(Ende.core.isSetIterator(set.values()));
  assert(!Ende.core.isSetIterator(set));
});

test(function testIsSharedArrayBuffer() {
  assert(Ende.core.isSharedArrayBuffer(new SharedArrayBuffer(4)));
  assert(!Ende.core.isSharedArrayBuffer(new ArrayBuffer(4)));
  assert(!Ende.core.isSharedArrayBuffer(new Uint8Array(4)));
});

test(function testIsStringObject() {
  assert(Ende.core.isStringObject(new String("str")));
  assert(!Ende.core.isStringObject("str"));
});

test(function testIsSymbolObject() {
  assert(Ende.core.isSymbolObject(Object(Symbol())));
  assert(!Ende.core.isSymbolObject(Symbol()));
});

test(function testIsTypedArray() {
  assert(Ende.core.isTypedArray(new Uint8Array(4)));
  assert(!Ende.core.isTypedArray(new DataView(new ArrayBuffer(4))));
  assert(!Ende.core.isTypedArray(new ArrayBuffer(4)));
});

test(function testIsWeakMap() {
  assert(Ende.core.isWeakMap(new WeakMap()));
  assert(!Ende.core.isWeakMap(new WeakSet()));
  assert(!Ende.core.isWeakMap(new Map()));
  assert(!Ende.core.isWeakMap(new Set()));
});

test(function testIsWeakSet() {
  assert(Ende.core.isWeakSet(new WeakSet()));
  assert(!Ende.core.isWeakSet(new WeakMap()));
  assert(!Ende.core.isWeakSet(new Set()));
  assert(!Ende.core.isWeakSet(new Map()));
});
