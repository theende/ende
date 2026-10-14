// Copyright 2018-2026 the Deno authors. MIT license.

import { trace } from "./otel_api.ts";

const tracer = trace.getTracer("example-tracer");

tracer.startActiveSpan("root", (root) => {
  tracer.startActiveSpan("child", (child) => {
    child.end();
  });
  root.end();
});
