// Changes the rule's grant or the package's manifest, as a user would.
const [what] = Ende.args;
if (what === "grant") {
  const config = JSON.parse(Ende.readTextFileSync("ende.json"));
  config.isolate["./vendor/m/"].permissions.read.push("./other");
  Ende.writeTextFileSync("ende.json", JSON.stringify(config));
} else if (what === "manifest") {
  // asks for more: env too
  const path = "vendor/m/mod.ts";
  Ende.writeTextFileSync(
    path,
    Ende.readTextFileSync(path).replace("read: true", "read: true, env: true"),
  );
} else if (what === "narrow") {
  const path = "vendor/m/mod.ts";
  Ende.writeTextFileSync(
    path,
    Ende.readTextFileSync(path).replace("read: true", "read: false"),
  );
}
