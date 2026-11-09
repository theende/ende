// Copyright 2018-2026 the Deno authors. MIT license.

/// <reference path="../../core/internal.d.ts" />

(function () {
const { core, primordials } = __bootstrap;
const {
  Error,
  ObjectDefineProperties,
  SafeWeakMap,
  Symbol,
  SymbolFor,
  SymbolToStringTag,
  TypeError,
  WeakMapPrototypeGet,
  WeakMapPrototypeSet,
} = primordials;

const { URL } = core.loadExtScript("ext:ende_web/00_url.js");
const { DOMException } = core.loadExtScript("ext:ende_web/01_dom_exception.js");

const locationConstructorKey = Symbol("locationConstructorKey");

// The differences between the definitions of `Location` and `WorkerLocation`
// are because of the `LegacyUnforgeable` attribute only specified upon
// `Location`'s properties. See:
// - https://html.spec.whatwg.org/multipage/history.html#the-location-interface
// - https://heycam.github.io/webidl/#LegacyUnforgeable
class Location {
  constructor(href = null, key = null) {
    if (key != locationConstructorKey) {
      throw new TypeError("Illegal constructor");
    }
    const url = new URL(href);
    url.username = "";
    url.password = "";
    ObjectDefineProperties(this, {
      hash: {
        __proto__: null,
        get() {
          return url.hash;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.hash"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      host: {
        __proto__: null,
        get() {
          return url.host;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.host"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      hostname: {
        __proto__: null,
        get() {
          return url.hostname;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.hostname"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      href: {
        __proto__: null,
        get() {
          return url.href;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.href"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      origin: {
        __proto__: null,
        get() {
          return url.origin;
        },
        enumerable: true,
      },
      pathname: {
        __proto__: null,
        get() {
          return url.pathname;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.pathname"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      port: {
        __proto__: null,
        get() {
          return url.port;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.port"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      protocol: {
        __proto__: null,
        get() {
          return url.protocol;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.protocol"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      search: {
        __proto__: null,
        get() {
          return url.search;
        },
        set() {
          throw new DOMException(
            `Cannot set "location.search"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      ancestorOrigins: {
        __proto__: null,
        get() {
          // TODO(nayeemrmn): Replace with a `DOMStringList` instance.
          return {
            length: 0,
            item: () => null,
            contains: () => false,
          };
        },
        enumerable: true,
      },
      assign: {
        __proto__: null,
        value: function assign() {
          throw new DOMException(
            `Cannot call "location.assign()"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      reload: {
        __proto__: null,
        value: function reload() {
          throw new DOMException(
            `Cannot call "location.reload()"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      replace: {
        __proto__: null,
        value: function replace() {
          throw new DOMException(
            `Cannot call "location.replace()"`,
            "NotSupportedError",
          );
        },
        enumerable: true,
      },
      toString: {
        __proto__: null,
        value: function toString() {
          return url.href;
        },
        enumerable: true,
      },
      [SymbolFor("Deno.privateCustomInspect")]: {
        __proto__: null,
        value: function (inspect, inspectOptions) {
          return `${this.constructor.name} ${
            inspect({
              hash: this.hash,
              host: this.host,
              hostname: this.hostname,
              href: this.href,
              origin: this.origin,
              pathname: this.pathname,
              port: this.port,
              protocol: this.protocol,
              search: this.search,
            }, inspectOptions)
          }`;
        },
      },
    });
  }
}

ObjectDefineProperties(Location.prototype, {
  [SymbolToStringTag]: {
    __proto__: null,
    value: "Location",
    configurable: true,
  },
});

const workerLocationUrls = new SafeWeakMap();

class WorkerLocation {
  constructor(href = null, key = null) {
    if (key != locationConstructorKey) {
      throw new TypeError("Illegal constructor");
    }
    const url = new URL(href);
    url.username = "";
    url.password = "";
    WeakMapPrototypeSet(workerLocationUrls, this, url);
  }
}

ObjectDefineProperties(WorkerLocation.prototype, {
  hash: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.hash;
    },
    configurable: true,
    enumerable: true,
  },
  host: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.host;
    },
    configurable: true,
    enumerable: true,
  },
  hostname: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.hostname;
    },
    configurable: true,
    enumerable: true,
  },
  href: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.href;
    },
    configurable: true,
    enumerable: true,
  },
  origin: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.origin;
    },
    configurable: true,
    enumerable: true,
  },
  pathname: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.pathname;
    },
    configurable: true,
    enumerable: true,
  },
  port: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.port;
    },
    configurable: true,
    enumerable: true,
  },
  protocol: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.protocol;
    },
    configurable: true,
    enumerable: true,
  },
  search: {
    __proto__: null,
    get() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.search;
    },
    configurable: true,
    enumerable: true,
  },
  toString: {
    __proto__: null,
    value: function toString() {
      const url = WeakMapPrototypeGet(workerLocationUrls, this);
      if (url == null) {
        throw new TypeError("Illegal invocation");
      }
      return url.href;
    },
    configurable: true,
    enumerable: true,
    writable: true,
  },
  [SymbolToStringTag]: {
    __proto__: null,
    value: "WorkerLocation",
    configurable: true,
  },
  [SymbolFor("Deno.privateCustomInspect")]: {
    __proto__: null,
    value: function (inspect, inspectOptions) {
      // A prototype or `Object.create(WorkerLocation.prototype)` has no URL.
      if (WeakMapPrototypeGet(workerLocationUrls, this) == null) {
        return `${this.constructor.name} ${inspect({}, inspectOptions)}`;
      }
      return `${this.constructor.name} ${
        inspect({
          hash: this.hash,
          host: this.host,
          hostname: this.hostname,
          href: this.href,
          origin: this.origin,
          pathname: this.pathname,
          port: this.port,
          protocol: this.protocol,
          search: this.search,
        }, inspectOptions)
      }`;
    },
  },
});

let location = undefined;
let workerLocation = undefined;

function setLocationHref(href) {
  location = new Location(href, locationConstructorKey);
  workerLocation = new WorkerLocation(href, locationConstructorKey);
}

function getLocationHref() {
  return location?.href;
}

const locationConstructorDescriptor = {
  __proto__: null,
  value: Location,
  configurable: true,
  writable: true,
};

const workerLocationConstructorDescriptor = {
  __proto__: null,
  value: WorkerLocation,
  configurable: true,
  writable: true,
};

// Like any Web IDL attribute getter of the global object, the `location`
// getters reject a receiver that isn't the global object.
function assertGlobalReceiver(receiver) {
  if (receiver !== undefined && receiver !== null && receiver !== globalThis) {
    throw new TypeError("Illegal invocation");
  }
}

const locationDescriptor = {
  __proto__: null,
  get() {
    assertGlobalReceiver(this);
    return location;
  },
  set() {
    throw new DOMException(`Cannot set "location"`, "NotSupportedError");
  },
  enumerable: true,
};
const workerLocationDescriptor = {
  __proto__: null,
  get() {
    assertGlobalReceiver(this);
    if (workerLocation == null) {
      throw new Error(
        `Assertion: "globalThis.location" must be defined in a worker`,
      );
    }
    return workerLocation;
  },
  configurable: true,
  enumerable: true,
};

// Not [Serializable]: structuredClone throws a DataCloneError.
{
  const { markNotSerializable } = core.loadExtScript(
    "ext:ende_web/13_message_port.js",
  );
  markNotSerializable(Location.prototype);
  markNotSerializable(WorkerLocation.prototype);
}

return {
  getLocationHref,
  locationConstructorDescriptor,
  locationDescriptor,
  setLocationHref,
  workerLocationConstructorDescriptor,
  workerLocationDescriptor,
};
})();
