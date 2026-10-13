// The denylisted Deno control variables must not have been imported from the
// env file...
console.log("ENDE_CONNECTED:", Deno.env.get("ENDE_CONNECTED"));
console.log(
  "ENDE_DEPLOY_TUNNEL_ENDPOINT:",
  Deno.env.get("ENDE_DEPLOY_TUNNEL_ENDPOINT"),
);
// ...but ordinary variables from the same file still are.
console.log("NOT_DENYLISTED:", Deno.env.get("NOT_DENYLISTED"));
console.log("user code started");
