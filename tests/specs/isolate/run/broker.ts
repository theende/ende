// Runs broker_app.ts under a permission broker that allows every request.
const socketPath = Ende.makeTempDirSync() + "/broker.sock";
const listener = Ende.listen({ transport: "unix", path: socketPath });
(async () => {
  for await (const conn of listener) {
    (async () => {
      const decoder = new TextDecoder();
      const buffer = new Uint8Array(4096);
      let pending = "";
      while (true) {
        const read = await conn.read(buffer);
        if (read === null) return;
        pending += decoder.decode(buffer.subarray(0, read));
        let newline;
        while ((newline = pending.indexOf("\n")) !== -1) {
          const request = JSON.parse(pending.slice(0, newline));
          pending = pending.slice(newline + 1);
          await conn.write(
            new TextEncoder().encode(
              JSON.stringify({ id: request.id, result: "allow" }) + "\n",
            ),
          );
        }
      }
    })().catch(() => {});
  }
})();
const output = await new Ende.Command(Ende.execPath(), {
  args: ["run", "--quiet", "broker_app.ts"],
  env: { ENDE_PERMISSION_BROKER_PATH: socketPath },
  stdout: "piped",
  stderr: "piped",
}).output();
console.log(new TextDecoder().decode(output.stdout).trim());
listener.close();
