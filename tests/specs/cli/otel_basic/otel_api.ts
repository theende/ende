// Copyright 2026 the Ende authors. MIT license.

// The parts of the `@opentelemetry/api` package these tests use, backed by
// Ende's built-in OpenTelemetry providers.

// deno-lint-ignore-file no-explicit-any

const SPAN_KEY = Symbol.for("OpenTelemetry Context Key SPAN");
const BAGGAGE_KEY = Symbol.for("OpenTelemetry Baggage Key");

function globalApi(): any {
  return (globalThis as any)[Symbol.for("opentelemetry.js.api.1")] ?? {};
}

export const context = {
  active(): any {
    return Ende.telemetry.contextManager.active();
  },
};

export const trace = {
  getTracer(name: string, version?: string, options?: unknown): any {
    return Ende.telemetry.tracerProvider.getTracer(name, version, options);
  },
  getActiveSpan(): any {
    return context.active().getValue(SPAN_KEY);
  },
};

export const metrics = {
  getMeter(name: string, version?: string, options?: unknown): any {
    return Ende.telemetry.meterProvider.getMeter(name, version, options);
  },
  setGlobalMeterProvider(_provider: unknown): boolean {
    // Ende's meter provider is always the global one.
    return true;
  },
};

const defaultGetter = {
  get(carrier: any, key: string) {
    return carrier[key];
  },
  keys(carrier: any) {
    return Object.keys(carrier);
  },
};

const defaultSetter = {
  set(carrier: any, key: string, value: string) {
    carrier[key] = value;
  },
};

export const propagation = {
  inject(ctx: any, carrier: unknown, setter: unknown = defaultSetter): void {
    globalApi().propagation?.inject(ctx, carrier, setter);
  },
  extract(ctx: any, carrier: unknown, getter: unknown = defaultGetter): any {
    const propagator = globalApi().propagation;
    return propagator ? propagator.extract(ctx, carrier, getter) : ctx;
  },
  getBaggage(ctx: any): any {
    return ctx.getValue(BAGGAGE_KEY);
  },
  getActiveBaggage(): any {
    return context.active().getValue(BAGGAGE_KEY);
  },
};

export interface ObservableResult {
  observe(value: number, attributes?: Record<string, unknown>): void;
}
