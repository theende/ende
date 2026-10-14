// Copyright 2026 the Ende authors. MIT license.

// Runs the script given as the first argument with OTLP gRPC export to the
// test server's collector on port 4267, which sends `grpc-status` in HTTP/2
// trailers (normal form) instead of in the initial response headers
// (Trailers-Only form). The status to send comes from GRPC_TRAILER_STATUS and
// GRPC_TRAILER_MESSAGE. Prints the exported data once the script exits.

import {
  decodeExportLogsRequest,
  decodeExportMetricsRequest,
  decodeExportTraceRequest,
} from "./proto_decode.ts";

const collector = "https://localhost:4267";
const session = crypto.randomUUID();
const grpcStatus = Ende.env.get("GRPC_TRAILER_STATUS") ?? "0";
const grpcMessage = Ende.env.get("GRPC_TRAILER_MESSAGE");

const headers = [
  `x-test-session=${session}`,
  `x-test-grpc-status=${grpcStatus}`,
];
if (grpcMessage) {
  headers.push(`x-test-grpc-message=${grpcMessage}`);
}

const status = await new Ende.Command(Ende.execPath(), {
  args: [
    "run",
    "--env-file=env_file",
    "-A",
    "-q",
    "--unstable-cron",
    Ende.args[0],
  ],
  env: {
    OTEL_EXPORTER_OTLP_ENDPOINT: collector,
    OTEL_EXPORTER_OTLP_PROTOCOL: "grpc",
    OTEL_EXPORTER_OTLP_HEADERS: headers.join(","),
    ...(grpcStatus !== "0" ? { ENDE_LOG: "error" } : {}),
  },
  stdout: "null",
}).spawn().status;
if (status.signal) {
  throw new Error("child process failed: " + JSON.stringify(status));
}

const client = Ende.createHttpClient({
  caCerts: [Ende.readTextFileSync("../../../testdata/tls/RootCA.pem")],
});
const res = await fetch(`${collector}/requests/${session}`, { client });
const requests: { path: string; body: string }[] = await res.json();
client.close();

// deno-lint-ignore no-explicit-any
const data: { spans: any[]; logs: any[]; metrics: any[] } = {
  spans: [],
  logs: [],
  metrics: [],
};
for (const { path, body } of requests) {
  // Strip the 5-byte gRPC frame prefix (compression flag + length)
  const bytes = Uint8Array.from(atob(body), (c) => c.charCodeAt(0)).slice(5);
  if (path.includes("TraceService")) {
    for (const r of decodeExportTraceRequest(bytes).resourceSpans ?? []) {
      for (const s of r.scopeSpans) data.spans.push(...s.spans);
    }
  } else if (path.includes("LogsService")) {
    for (const r of decodeExportLogsRequest(bytes).resourceLogs ?? []) {
      for (const s of r.scopeLogs) data.logs.push(...s.logRecords);
    }
  } else if (path.includes("MetricsService")) {
    for (const r of decodeExportMetricsRequest(bytes).resourceMetrics ?? []) {
      for (const s of r.scopeMetrics) data.metrics.push(...s.metrics);
    }
  }
}

data.logs.sort((a, b) =>
  Number(BigInt(a.observedTimeUnixNano) - BigInt(b.observedTimeUnixNano))
);
data.spans.sort((a, b) =>
  Number(BigInt(`0x${a.spanId}`) - BigInt(`0x${b.spanId}`))
);
// v8js metrics are non-deterministic
data.metrics = data.metrics.filter((m) => !m.name.startsWith("v8js"));
data.metrics.sort((a, b) => a.name.localeCompare(b.name));
console.log(JSON.stringify(data, null, 2));
