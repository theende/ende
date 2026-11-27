// Changes the config, the package or the app, as a user would.
const [what] = Ende.args;
const replace = (path: string, from: string, to: string) => {
  const text = Ende.readTextFileSync(path);
  if (!text.includes(from)) throw new Error(`${from} not in ${path}`);
  Ende.writeTextFileSync(path, text.replace(from, to));
};
switch (what) {
  case "grant":
    replace("ende.json", '"./data"]', '"./data", "./other"]');
    break;
  case "unmanifest":
    replace(
      "vendor/m/mod.ts",
      "export const manifest = { permissions: { read: true } };",
      "",
    );
    break;
  case "respell":
    replace("ende.json", '"./vendor/m/"', '"./vendor//m/"');
    break;
  case "respell_wider":
    replace("ende.json", '"./vendor//m/"', '"./vendor/./m/"');
    replace("ende.json", '"env": true', '"env": true, "sys": true');
    break;
  case "respell_case":
    replace("ende.json", '"./vendor/m/"', '"./Vendor/m/"');
    replace("ende.json", '"env": true', '"env": true, "sys": true');
    break;
  case "overlap":
    replace(
      "ende.json",
      '"isolate": {',
      '"isolate": {\n    "./vendor/": { "permissions": { "read": ["./data"], "env": true } },',
    );
    break;
  case "rename":
    Ende.renameSync("vendor/m/mod.ts", "vendor/m/index.ts");
    replace("main.ts", "./vendor/m/mod.ts", "./vendor/m/index.ts");
    break;
  default:
    throw new Error(`unknown edit ${what}`);
}
