// Changes the config or the package, as a user would.
const [what] = Ende.args;
switch (what) {
  case "remove_rule":
    Ende.writeTextFileSync("ende.json", "{}\n");
    break;
  case "narrow": {
    const path = "vendor/m/mod.ts";
    Ende.writeTextFileSync(
      path,
      Ende.readTextFileSync(path).replace("{ read: true }", "{ read: false }"),
    );
    break;
  }
  default:
    throw new Error(`unknown edit ${what}`);
}
