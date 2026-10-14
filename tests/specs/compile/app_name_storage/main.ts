// Storage identity is driven by the app name, not the binary file name:
// binaries sharing an `--app-name` share a store; differing names do not.
if (Deno.args[0] === "set") {
  localStorage.setItem("greeting", "hi deno team.");
  console.log("set");
} else {
  console.log("value:", localStorage.getItem("greeting"));
}
