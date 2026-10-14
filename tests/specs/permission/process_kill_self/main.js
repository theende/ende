// Signal 0 is a "is this process alive" check that does not actually
// deliver a signal. Sending it to ourselves should not require
// --allow-run.
Deno.kill(Deno.pid, 0);
console.log("ok");
