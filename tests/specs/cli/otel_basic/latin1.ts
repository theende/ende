// Copyright 2018-2026 the Deno authors. MIT license.

import { trace } from "./otel_api.ts";

// V8 stores strings whose characters all fit in Latin-1 as one-byte strings.
console.log("latin1-only: Médicale");
console.log("with-euro: Médicale €");

const tracer = trace.getTracer("example-tracer");
const span = tracer.startSpan("latin1 span");
span.setAttribute("clé", "café");
span.setAttribute("noms", ["Zoë", "Ñu"]);
span.end();
