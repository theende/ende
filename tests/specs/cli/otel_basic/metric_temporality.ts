import { metrics } from "./otel_api.ts";

const meter = metrics.getMeter("delta_test");

const counter = meter.createCounter("delta_counter", {
  description: "Counter with delta temporality",
});
counter.add(1);

setTimeout(() => counter.add(1), 2000);
