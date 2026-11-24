// Each mode does one thing through a root on `data/` and prints the result,
// or the error's class and message.
const [mode] = Deno.args;

async function run(): Promise<string> {
  switch (mode) {
    case "link": {
      // A symlink inside the root to the denied subdirectory.
      await Deno.symlink("secret/s.txt", "data/link");
      return "linked";
    }
    case "read": {
      await using root = await Deno.openRoot("data");
      return (await root.readTextFile("a.txt")).trim();
    }
    case "read_secret": {
      await using root = await Deno.openRoot("data");
      return (await root.readTextFile("secret/s.txt")).trim();
    }
    case "read_link": {
      await using root = await Deno.openRoot("data");
      return (await root.readTextFile("link")).trim();
    }
    case "stat_link": {
      using root = Deno.openRootSync("data");
      return String(root.statSync("link").size);
    }
    case "revoke": {
      await using root = await Deno.openRoot("data");
      await Deno.permissions.revoke({ name: "read", path: "data" });
      return (await root.readTextFile("a.txt")).trim();
    }
    case "open_write": {
      await using root = await Deno.openRoot("data", { write: true });
      return "opened";
    }
    case "write_locked": {
      await using root = await Deno.openRoot("data", { write: true });
      await root.writeTextFile("locked/new.txt", "x");
      return "written";
    }
    case "write": {
      await using root = await Deno.openRoot("data", { write: true });
      await root.writeTextFile("new.txt", "x");
      return "written";
    }
    case "utime": {
      using root = Deno.openRootSync("data");
      using file = root.openSync("a.txt");
      file.utimeSync(0, 0);
      return "utimed";
    }
    case "utime_revoke": {
      using root = Deno.openRootSync("data");
      using file = root.openSync("a.txt");
      await Deno.permissions.revoke({ name: "write" });
      file.utimeSync(0, 0);
      return "utimed";
    }
    case "fstat_write_only": {
      using root = Deno.openRootSync("data", { read: false, write: true });
      using file = root.openSync("a.txt", { write: true });
      return String(file.statSync().size);
    }
    case "read_secret_missing": {
      await using root = await Deno.openRoot("data");
      return (await root.readTextFile("secret/nope")).trim();
    }
    case "mkfifo": {
      new Deno.Command("mkfifo", { args: ["data/secret/f"] }).outputSync();
      return "made";
    }
    case "read_fifo": {
      // A regression would block in open() waiting for a writer: report it
      // instead of hanging the test.
      const timer = setTimeout(() => {
        console.log("blocked");
        Deno.exit(0);
      }, 5000);
      try {
        await using root = await Deno.openRoot("data");
        return String((await root.readFile("secret/f")).length);
      } finally {
        clearTimeout(timer);
      }
    }
    case "mkdir_existing": {
      using root = Deno.openRootSync("data", { write: true });
      root.mkdirSync("locked/a", { recursive: true });
      return "made";
    }
    case "rename_d": {
      using root = Deno.openRootSync("data", { write: true });
      root.renameSync("d", "d2");
      return "renamed";
    }
    case "plain_rename_d": {
      Deno.renameSync("data/d", "data/d2");
      return "renamed";
    }
    case "plain_rename_d_async": {
      await Deno.rename("data/d", "data/d2");
      return "renamed";
    }
  }
  throw new Error(`unknown mode ${mode}`);
}

try {
  console.log(await run());
} catch (e) {
  console.log(`${(e as Error).constructor.name}: ${(e as Error).message}`);
}
