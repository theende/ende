// Copyright 2018-2026 the Deno authors. MIT license.

// @ts-check
/// <reference path="../../core/lib.deno_core.d.ts" />
/// <reference path="../web/internal.d.ts" />
/// <reference path="../../cli/tsc/dts/lib.ende_web.d.ts" />
/// <reference path="../../cli/tsc/dts/lib.ende_webgpu.d.ts" />

import { core, primordials } from "ext:core/mod.js";
import {
  GPU,
  GPUAdapter,
  GPUAdapterInfo,
  GPUBindGroup,
  GPUBindGroupLayout,
  GPUBuffer,
  GPUCanvasContext,
  GPUCommandBuffer,
  GPUCommandEncoder,
  GPUCompilationInfo,
  GPUCompilationMessage,
  GPUComputePassEncoder,
  GPUComputePipeline,
  GPUDevice,
  GPUDeviceLostInfo,
  GPUExternalTexture,
  GPUPipelineLayout,
  GPUQuerySet,
  GPUQueue,
  GPURenderBundle,
  GPURenderBundleEncoder,
  GPURenderPassEncoder,
  GPURenderPipeline,
  GPUSampler,
  GPUShaderModule,
  GPUSupportedFeatures,
  GPUSupportedLimits,
  GPUTexture,
  GPUTextureView,
  op_create_gpu,
  op_webgpu_device_start_capture,
  op_webgpu_device_stop_capture,
  WGSLLanguageFeatures,
} from "ext:core/ops";
const {
  ObjectDefineProperty,
  ObjectEntries,
  ObjectFreeze,
  ObjectPrototypeIsPrototypeOf,
  ObjectSetPrototypeOf,
  SafeArrayIterator,
  Symbol,
  SymbolFor,
  SymbolToStringTag,
} = primordials;

// Shared default for omitted options, which are only read. `undefined` would
// change the error thrown for the required `reason` member.
const EMPTY_OPTIONS = ObjectFreeze(ObjectSetPrototypeOf({}, null));

const webidl = core.loadExtScript("ext:ende_webidl/00_webidl.js");
const {
  defineEventHandler,
  Event,
  EventTargetPrototype,
  setEventTargetData,
} = core.loadExtScript("ext:ende_web/02_event.js");
const { DOMException, serializeSubclass, setClonedStack } = core.loadExtScript(
  "ext:ende_web/01_dom_exception.js",
);
const { createFilteredInspectProxy } = core.loadExtScript(
  "ext:ende_web/01_console.js",
);

const privateCustomInspect = SymbolFor("Deno.privateCustomInspect");
const _message = Symbol("[[message]]");
const illegalConstructorKey = Symbol("illegalConstructorKey");

class GPUError {
  constructor(key = null) {
    if (key !== illegalConstructorKey) {
      webidl.illegalConstructor();
    }
  }

  [_message];
  get message() {
    webidl.assertBranded(this, GPUErrorPrototype);
    return this[_message];
  }

  [privateCustomInspect](inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUErrorPrototype, this),
        keys: [
          "message",
        ],
      }),
      inspectOptions,
    );
  }
}
const GPUErrorPrototype = GPUError.prototype;

class GPUValidationError extends GPUError {
  /** @param {string} message */
  constructor(message) {
    const prefix = "Failed to construct 'GPUValidationError'";
    webidl.requiredArguments(arguments.length, 1, prefix);
    message = webidl.converters.DOMString(message, prefix, "Argument 1");
    super(illegalConstructorKey);
    this[webidl.brand] = webidl.brand;
    this[_message] = message;
  }
}
core.registerErrorClass("GPUValidationError", GPUValidationError);

class GPUOutOfMemoryError extends GPUError {
  constructor(message) {
    const prefix = "Failed to construct 'GPUOutOfMemoryError'";
    webidl.requiredArguments(arguments.length, 1, prefix);
    message = webidl.converters.DOMString(message, prefix, "Argument 1");
    super(illegalConstructorKey);
    this[webidl.brand] = webidl.brand;
    this[_message] = message;
  }
}
core.registerErrorClass("GPUOutOfMemoryError", GPUOutOfMemoryError);

class GPUInternalError extends GPUError {
  constructor() {
    super(illegalConstructorKey);
    this[webidl.brand] = webidl.brand;
  }
}
core.registerErrorClass("GPUInternalError", GPUInternalError);

class GPUPipelineError extends DOMException {
  #reason;

  constructor(message = "", options = EMPTY_OPTIONS) {
    const prefix = "Failed to construct 'GPUPipelineError'";
    message = webidl.converters.DOMString(message, prefix, "Argument 1");
    options = webidl.converters.GPUPipelineErrorInit(
      options,
      prefix,
      "Argument 2",
    );
    super(message, "GPUPipelineError");

    this.#reason = options.reason;
  }

  get reason() {
    webidl.assertBranded(this, GPUPipelineErrorPrototype);
    return this.#reason;
  }

  // https://gpuweb.github.io/gpuweb/#gpupipelineerror (serialization steps)
  [serializeSubclass](data) {
    data.type = "GPUPipelineError";
    data.reason = this.#reason;
  }
}
const GPUPipelineErrorPrototype = GPUPipelineError.prototype;

// Registered as a cloneable resource by ext:ende_web/13_message_port.js, so a
// realm that hasn't loaded this module can receive one.
function deserializeGPUPipelineError(data) {
  return setClonedStack(
    new GPUPipelineError(data.message, {
      __proto__: null,
      reason: data.reason,
    }),
    data,
  );
}

class GPUUncapturedErrorEvent extends Event {
  #error;

  constructor(type, gpuUncapturedErrorEventInitDict) {
    super(type, gpuUncapturedErrorEventInitDict);
    this[webidl.brand] = webidl.brand;

    const prefix = "Failed to construct 'GPUUncapturedErrorEvent'";
    webidl.requiredArguments(arguments.length, 2, prefix);
    gpuUncapturedErrorEventInitDict = webidl.converters
      .GPUUncapturedErrorEventInit(
        gpuUncapturedErrorEventInitDict,
        prefix,
        "Argument 2",
      );

    this.#error = gpuUncapturedErrorEventInitDict.error;
  }

  get error() {
    webidl.assertBranded(this, GPUUncapturedErrorEventPrototype);
    return this.#error;
  }
}
const GPUUncapturedErrorEventPrototype = GPUUncapturedErrorEvent.prototype;

const GPUPrototype = GPU.prototype;
ObjectDefineProperty(GPUPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return `${this.constructor.name} ${inspect({}, inspectOptions)}`;
  },
});

const GPUAdapterPrototype = GPUAdapter.prototype;
ObjectDefineProperty(GPUAdapterPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUAdapterPrototype, this),
        keys: [
          "features",
          "limits",
          "info",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUAdapterInfoPrototype = GPUAdapterInfo.prototype;
ObjectDefineProperty(GPUAdapterInfoPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUAdapterInfoPrototype, this),
        keys: [
          "vendor",
          "architecture",
          "device",
          "description",
          "subgroupMinSize",
          "subgroupMaxSize",
          "isFallbackAdapter",
        ],
      }),
      inspectOptions,
    );
  },
});

// A subclass prototype or `Object.create(X.prototype)` inherits from the
// prototype but has no set behind it, so iterating it throws.
function inspectSetlike(self, prototype, inspect, inspectOptions) {
  let values = {};
  if (ObjectPrototypeIsPrototypeOf(prototype, self)) {
    try {
      // deno-lint-ignore ende-internal/prefer-primordials
      values = [...self];
    } catch {
      // Not a real instance: show it like the prototype.
    }
  }
  return `${self.constructor.name} ${inspect(values, inspectOptions)}`;
}

const GPUSupportedFeaturesPrototype = GPUSupportedFeatures.prototype;
webidl.setlikeObjectWrap(GPUSupportedFeaturesPrototype, true);
ObjectDefineProperty(GPUSupportedFeaturesPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspectSetlike(
      this,
      GPUSupportedFeaturesPrototype,
      inspect,
      inspectOptions,
    );
  },
});

const WGSLLanguageFeaturesPrototype = WGSLLanguageFeatures.prototype;
webidl.setlikeObjectWrap(WGSLLanguageFeaturesPrototype, true);
ObjectDefineProperty(WGSLLanguageFeaturesPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspectSetlike(
      this,
      WGSLLanguageFeaturesPrototype,
      inspect,
      inspectOptions,
    );
  },
});

const GPUSupportedLimitsPrototype = GPUSupportedLimits.prototype;
ObjectDefineProperty(GPUSupportedLimitsPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUSupportedLimitsPrototype,
          this,
        ),
        keys: [
          "maxTextureDimension1D",
          "maxTextureDimension2D",
          "maxTextureDimension3D",
          "maxTextureArrayLayers",
          "maxBindGroups",
          // TODO(@crowlKats): support max_bind_groups_plus_vertex_buffers
          // "maxBindGroupsPlusVertexBuffers",
          "maxBindingsPerBindGroup",
          "maxDynamicUniformBuffersPerPipelineLayout",
          "maxDynamicStorageBuffersPerPipelineLayout",
          "maxSampledTexturesPerShaderStage",
          "maxSamplersPerShaderStage",
          "maxStorageBuffersPerShaderStage",
          "maxStorageTexturesPerShaderStage",
          "maxUniformBuffersPerShaderStage",
          "maxUniformBufferBindingSize",
          "maxStorageBufferBindingSize",
          "minUniformBufferOffsetAlignment",
          "minStorageBufferOffsetAlignment",
          "maxVertexBuffers",
          "maxBufferSize",
          "maxVertexAttributes",
          "maxVertexBufferArrayStride",
          "maxInterStageShaderVariables",
          "maxColorAttachments",
          "maxColorAttachmentBytesPerSample",
          "maxComputeWorkgroupStorageSize",
          "maxComputeInvocationsPerWorkgroup",
          "maxComputeWorkgroupSizeX",
          "maxComputeWorkgroupSizeY",
          "maxComputeWorkgroupSizeZ",
          "maxComputeWorkgroupsPerDimension",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUDeviceLostInfoPrototype = GPUDeviceLostInfo.prototype;
ObjectDefineProperty(GPUDeviceLostInfoPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUDeviceLostInfoPrototype,
          this,
        ),
        keys: [
          "reason",
          "message",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUDevicePrototype = GPUDevice.prototype;
ObjectSetPrototypeOf(GPUDevicePrototype, EventTargetPrototype);
defineEventHandler(GPUDevicePrototype, "uncapturederror");
ObjectDefineProperty(GPUDevicePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUDevicePrototype, this),
        keys: [
          "features",
          "label",
          "limits",
          "lost",
          "queue",
          // TODO(lucacasonato): emit an UncapturedErrorEvent
          // "onuncapturederror"
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUQueuePrototype = GPUQueue.prototype;
ObjectDefineProperty(GPUQueuePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUQueuePrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUBufferPrototype = GPUBuffer.prototype;
ObjectDefineProperty(GPUBufferPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUBufferPrototype, this),
        keys: [
          "label",
          "mapState",
          "size",
          "usage",
        ],
      }),
      inspectOptions,
    );
  },
});

// A Web IDL namespace of constants (GPUBufferUsage, ...): a plain object with
// its own toStringTag and read-only, enumerable, non-configurable members.
function createNamespace(name, constants) {
  const namespace = {};
  for (
    const { 0: key, 1: value } of new SafeArrayIterator(
      ObjectEntries(constants),
    )
  ) {
    ObjectDefineProperty(namespace, key, {
      __proto__: null,
      value,
      writable: false,
      enumerable: true,
      configurable: false,
    });
  }
  ObjectDefineProperty(namespace, SymbolToStringTag, {
    __proto__: null,
    value: name,
    writable: false,
    enumerable: false,
    configurable: true,
  });
  return namespace;
}

const GPUBufferUsage = createNamespace("GPUBufferUsage", {
  MAP_READ: 0x0001,
  MAP_WRITE: 0x0002,
  COPY_SRC: 0x0004,
  COPY_DST: 0x0008,
  INDEX: 0x0010,
  VERTEX: 0x0020,
  UNIFORM: 0x0040,
  STORAGE: 0x0080,
  INDIRECT: 0x0100,
  QUERY_RESOLVE: 0x0200,
});

const GPUMapMode = createNamespace("GPUMapMode", {
  READ: 0x0001,
  WRITE: 0x0002,
});

const GPUTexturePrototype = GPUTexture.prototype;
ObjectDefineProperty(GPUTexturePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUTexturePrototype, this),
        keys: [
          "label",
          "width",
          "height",
          "depthOrArrayLayers",
          "mipLevelCount",
          "sampleCount",
          "dimension",
          "format",
          "usage",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUTextureUsage = createNamespace("GPUTextureUsage", {
  COPY_SRC: 0x01,
  COPY_DST: 0x02,
  TEXTURE_BINDING: 0x04,
  STORAGE_BINDING: 0x08,
  RENDER_ATTACHMENT: 0x10,
});

const GPUTextureViewPrototype = GPUTextureView.prototype;
ObjectDefineProperty(GPUTextureViewPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUTextureViewPrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUSamplerPrototype = GPUSampler.prototype;
ObjectDefineProperty(GPUSamplerPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUSamplerPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUBindGroupLayoutPrototype = GPUBindGroupLayout.prototype;
ObjectDefineProperty(GPUBindGroupLayoutPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUBindGroupLayoutPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUPipelineLayoutPrototype = GPUPipelineLayout.prototype;
ObjectDefineProperty(GPUPipelineLayoutPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUPipelineLayoutPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUBindGroupPrototype = GPUBindGroup.prototype;
ObjectDefineProperty(GPUBindGroupPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUBindGroupPrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUShaderModulePrototype = GPUShaderModule.prototype;
ObjectDefineProperty(GPUShaderModulePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUShaderModulePrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUCompilationInfoPrototype = GPUCompilationInfo.prototype;
ObjectDefineProperty(GPUCompilationInfoPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUCompilationInfoPrototype,
          this,
        ),
        keys: [
          "messages",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUCompilationMessagePrototype = GPUCompilationMessage.prototype;
ObjectDefineProperty(GPUCompilationMessagePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUCompilationMessagePrototype,
          this,
        ),
        keys: [
          "message",
          "type",
          "lineNum",
          "linePos",
          "offset",
          "length",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUShaderStage = createNamespace("GPUShaderStage", {
  VERTEX: 0x1,
  FRAGMENT: 0x2,
  COMPUTE: 0x4,
});

const GPUComputePipelinePrototype = GPUComputePipeline.prototype;
ObjectDefineProperty(GPUComputePipelinePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUComputePipelinePrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPURenderPipelinePrototype = GPURenderPipeline.prototype;
ObjectDefineProperty(GPURenderPipelinePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPURenderPipelinePrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUColorWrite = createNamespace("GPUColorWrite", {
  RED: 0x1,
  GREEN: 0x2,
  BLUE: 0x4,
  ALPHA: 0x8,
  ALL: 0xF,
});

const GPUCommandEncoderPrototype = GPUCommandEncoder.prototype;
ObjectDefineProperty(GPUCommandEncoderPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUCommandEncoderPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPURenderPassEncoderPrototype = GPURenderPassEncoder.prototype;
ObjectDefineProperty(GPURenderPassEncoderPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPURenderPassEncoderPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUComputePassEncoderPrototype = GPUComputePassEncoder.prototype;
ObjectDefineProperty(GPUComputePassEncoderPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPUComputePassEncoderPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUCommandBufferPrototype = GPUCommandBuffer.prototype;
ObjectDefineProperty(GPUCommandBufferPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUCommandBufferPrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPURenderBundleEncoderPrototype = GPURenderBundleEncoder.prototype;
ObjectDefineProperty(GPURenderBundleEncoderPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(
          GPURenderBundleEncoderPrototype,
          this,
        ),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPURenderBundlePrototype = GPURenderBundle.prototype;
ObjectDefineProperty(GPURenderBundlePrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPURenderBundlePrototype, this),
        keys: [
          "label",
        ],
      }),
      inspectOptions,
    );
  },
});

const GPUQuerySetPrototype = GPUQuerySet.prototype;
ObjectDefineProperty(GPUQuerySetPrototype, privateCustomInspect, {
  __proto__: null,
  value(inspect, inspectOptions) {
    return inspect(
      createFilteredInspectProxy({
        object: this,
        evaluate: ObjectPrototypeIsPrototypeOf(GPUQuerySetPrototype, this),
        keys: [
          "label",
          "type",
          "count",
        ],
      }),
      inspectOptions,
    );
  },
});

// https://webidl.spec.whatwg.org/#interface-prototype-object
for (
  const iface of new SafeArrayIterator([
    GPU,
    GPUAdapter,
    GPUAdapterInfo,
    GPUBindGroup,
    GPUBindGroupLayout,
    GPUBuffer,
    GPUCanvasContext,
    GPUCommandBuffer,
    GPUCommandEncoder,
    GPUCompilationInfo,
    GPUCompilationMessage,
    GPUComputePassEncoder,
    GPUComputePipeline,
    GPUDevice,
    GPUDeviceLostInfo,
    GPUError,
    GPUExternalTexture,
    GPUInternalError,
    GPUOutOfMemoryError,
    GPUPipelineError,
    GPUPipelineLayout,
    GPUQuerySet,
    GPUQueue,
    GPURenderBundle,
    GPURenderBundleEncoder,
    GPURenderPassEncoder,
    GPURenderPipeline,
    GPUSampler,
    GPUShaderModule,
    GPUSupportedFeatures,
    GPUSupportedLimits,
    GPUTexture,
    GPUTextureView,
    GPUUncapturedErrorEvent,
    GPUValidationError,
    WGSLLanguageFeatures,
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

// Converters

webidl.converters["GPUPipelineErrorReason"] = webidl.createEnumConverter(
  "GPUPipelineErrorReason",
  [
    "validation",
    "internal",
  ],
);

webidl.converters["GPUPipelineErrorInit"] = webidl.createDictionaryConverter(
  "GPUPipelineErrorInit",
  [
    {
      key: "reason",
      converter: webidl.converters.GPUPipelineErrorReason,
      required: true,
    },
  ],
);

webidl.converters["GPUError"] = webidl.converters.any /* put union here! */;

const dictMembersGPUUncapturedErrorEventInit = [
  { key: "error", converter: webidl.converters["GPUError"], required: true },
];
webidl.converters["GPUUncapturedErrorEventInit"] = webidl
  .createDictionaryConverter(
    "GPUUncapturedErrorEventInit",
    // dictMembersEventInit,
    dictMembersGPUUncapturedErrorEventInit,
  );

function deviceStartCapture(device) {
  op_webgpu_device_start_capture(device);
}

function deviceStopCapture(device) {
  op_webgpu_device_stop_capture(device);
}

const endeNsWebGPU = {
  deviceStartCapture,
  deviceStopCapture,
};

let gpu;
function initGPU() {
  if (!gpu) {
    gpu = op_create_gpu(
      webidl.brand,
      setEventTargetData,
      GPUUncapturedErrorEvent,
      GPUPipelineError,
    );
  }
}

// Not [Serializable] (GPUPipelineError is, and GPUDevice is an EventTarget):
// structuredClone throws a DataCloneError.
{
  const { markNotSerializable } = core.loadExtScript(
    "ext:ende_web/13_message_port.js",
  );
  for (
    const iface of new SafeArrayIterator([
      GPU,
      GPUAdapter,
      GPUAdapterInfo,
      GPUBindGroup,
      GPUBindGroupLayout,
      GPUBuffer,
      GPUCanvasContext,
      GPUCommandBuffer,
      GPUCommandEncoder,
      GPUCompilationInfo,
      GPUCompilationMessage,
      GPUComputePassEncoder,
      GPUComputePipeline,
      GPUDeviceLostInfo,
      GPUError,
      GPUInternalError,
      GPUOutOfMemoryError,
      GPUPipelineLayout,
      GPUQuerySet,
      GPUQueue,
      GPURenderBundle,
      GPURenderBundleEncoder,
      GPURenderPassEncoder,
      GPURenderPipeline,
      GPUSampler,
      GPUShaderModule,
      GPUSupportedFeatures,
      GPUSupportedLimits,
      GPUTexture,
      GPUTextureView,
      GPUValidationError,
      WGSLLanguageFeatures,
    ])
  ) {
    markNotSerializable(iface.prototype);
  }
}

export {
  deserializeGPUPipelineError,
  endeNsWebGPU,
  GPU,
  gpu,
  GPUAdapter,
  GPUAdapterInfo,
  GPUBindGroup,
  GPUBindGroupLayout,
  GPUBuffer,
  GPUBufferUsage,
  GPUCanvasContext,
  GPUColorWrite,
  GPUCommandBuffer,
  GPUCommandEncoder,
  GPUCompilationInfo,
  GPUCompilationMessage,
  GPUComputePassEncoder,
  GPUComputePipeline,
  GPUDevice,
  GPUDeviceLostInfo,
  GPUError,
  GPUExternalTexture,
  GPUInternalError,
  GPUMapMode,
  GPUOutOfMemoryError,
  GPUPipelineError,
  GPUPipelineLayout,
  GPUQuerySet,
  GPUQueue,
  GPURenderBundle,
  GPURenderBundleEncoder,
  GPURenderPassEncoder,
  GPURenderPipeline,
  GPUSampler,
  GPUShaderModule,
  GPUShaderStage,
  GPUSupportedFeatures,
  GPUSupportedLimits,
  GPUTexture,
  GPUTextureUsage,
  GPUTextureView,
  GPUUncapturedErrorEvent,
  GPUValidationError,
  initGPU,
  WGSLLanguageFeatures,
};
