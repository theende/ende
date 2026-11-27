// Changes the package or the app, as a user would.
const [what] = Ende.args;
switch (what) {
  case "rename_and_unmanifest": {
    const text = Ende.readTextFileSync("vendor/m/mod.ts");
    Ende.removeSync("vendor/m/mod.ts");
    Ende.writeTextFileSync(
      "vendor/m/index.ts",
      text.replace(
        "export const manifest = { permissions: { read: true } };",
        "",
      ),
    );
    Ende.writeTextFileSync(
      "main.ts",
      Ende.readTextFileSync("main.ts").replace("mod.ts", "index.ts"),
    );
    break;
  }
  case "link_m1":
    Ende.symlinkSync("pkgs/m-1", "linked");
    break;
  case "link_m2":
    Ende.removeSync("linked");
    Ende.symlinkSync("pkgs/m-2", "linked");
    break;
  default:
    throw new Error(`unknown edit ${what}`);
}
