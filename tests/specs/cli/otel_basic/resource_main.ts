// Copyright 2018-2026 the Deno authors. MIT license.

// Runs `resource.ts` with OTel and permission auditing enabled, and prints the
// resource attributes, instrumentation scopes and permission audit attribute
// keys the runtime reports.

const resource: Record<string, unknown> = {};
const scopes = new Set<string>();
const permissionKeys = new Set<string>();

function collect(resourceAttrs, scope, attrs = []) {
  for (const { key, value } of resourceAttrs ?? []) {
    if (key.startsWith("process.runtime.") || key.startsWith("telemetry.")) {
      resource[key] = key.endsWith("version") ? "[version]" : value.stringValue;
    }
  }
  if (scope?.name) scopes.add(scope.name);
  for (const { key } of attrs) {
    if (key.includes("permission")) permissionKeys.add(key);
  }
}

const server = Ende.serve({
  key: Ende.readTextFileSync("../../../testdata/tls/localhost.key"),
  cert: Ende.readTextFileSync("../../../testdata/tls/localhost.crt"),
  port: 0,
  onListen({ port }) {
    const command = new Ende.Command(Ende.execPath(), {
      args: ["run", "--env-file=env_file", "-A", "-q", "resource.ts"],
      env: {
        OTEL_EXPORTER_OTLP_ENDPOINT: `https://localhost:${port}`,
        ENDE_AUDIT_PERMISSIONS: "otel",
      },
      stdout: "null",
    });
    command.spawn().status
      .then(() => server.shutdown())
      .then(() => {
        console.log(JSON.stringify(
          {
            resource: Object.fromEntries(
              Object.entries(resource).sort(([a], [b]) => a.localeCompare(b)),
            ),
            scopes: [...scopes].sort(),
            permissionKeys: [...permissionKeys].sort(),
          },
          null,
          2,
        ));
      });
  },
  async handler(req) {
    const body = await req.json();
    for (const r of body.resourceSpans ?? []) {
      for (const s of r.scopeSpans) {
        for (const span of s.spans) {
          collect(r.resource?.attributes, s.scope, span.attributes);
        }
      }
    }
    for (const r of body.resourceLogs ?? []) {
      for (const s of r.scopeLogs) {
        for (const log of s.logRecords) {
          collect(r.resource?.attributes, s.scope, log.attributes);
        }
      }
    }
    return Response.json({ partialSuccess: {} }, { status: 200 });
  },
});
