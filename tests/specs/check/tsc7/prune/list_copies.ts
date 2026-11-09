// Prints the copies in the tsc7 project directories under ENDE_DIR, relative
// to their project directory.
const projects = `${Ende.env.get("ENDE_DIR")}/tsc7_projects`;

function walk(dir: string, prefix: string, out: string[]) {
  for (const entry of Ende.readDirSync(dir)) {
    const name = `${prefix}${entry.name}`;
    if (entry.isDirectory) {
      walk(`${dir}/${entry.name}`, `${name}/`, out);
    } else {
      out.push(name);
    }
  }
}

const copies: string[] = [];
for (const project of Ende.readDirSync(projects)) {
  for (const kind of ["remote", "local"]) {
    try {
      walk(`${projects}/${project.name}/${kind}`, `${kind}/`, copies);
    } catch (err) {
      if (!(err instanceof Ende.errors.NotFound)) throw err;
    }
  }
}
console.log(copies.sort());
