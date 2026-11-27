// Changes the rules, as a user would.
const [what] = Ende.args;
const outer =
  '"./vendor/": { "permissions": { "read": ["./data"], "env": true } }';
const inner = '"./vendor/m/": { "permissions": [] }';
const outerRead = '"./vendor/": { "permissions": { "read": ["./data"] } }';
const write = (rules: string[]) =>
  Ende.writeTextFileSync(
    "ende.json",
    `{ "isolate": { ${rules.join(", ")} } }\n`,
  );
switch (what) {
  case "add_inner":
    write([inner, outer]);
    break;
  case "remove_inner":
    write([outer]);
    break;
  case "reorder":
    write([outer, inner]);
    break;
  case "read_after":
    write([inner, outerRead]);
    break;
  case "read_first":
    write([outerRead, inner]);
    break;
  case "inner_only":
    write([inner]);
    break;
  case "link_other":
    Ende.symlinkSync("vendor/other", "link");
    break;
  case "link_m":
    Ende.removeSync("link");
    Ende.symlinkSync("vendor/m", "link");
    break;
  default:
    throw new Error(`unknown edit ${what}`);
}
