// Copyright 2018-2026 the Ende authors. MIT license.
import {
  assert,
  assertEquals,
  assertRejects,
  assertThrows,
} from "./test_util.ts";

const isWindows = Deno.build.os === "windows";

/** A temp dir laid out as `root/` (with `a.txt` and `sub/b.txt`) beside an
 * `outside.txt` that names under the root must not reach. */
function setup(): { dir: string; root: string; outside: string } {
  const dir = Deno.makeTempDirSync();
  const root = `${dir}/root`;
  Deno.mkdirSync(`${root}/sub`, { recursive: true });
  Deno.writeTextFileSync(`${root}/a.txt`, "a");
  Deno.writeTextFileSync(`${root}/sub/b.txt`, "b");
  const outside = `${dir}/outside.txt`;
  Deno.writeTextFileSync(outside, "outside");
  return { dir, root, outside };
}

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootReadsAndWritesInside() {
    const { root } = setup();
    const r = await Deno.openRoot(root, { write: true });
    try {
      assertEquals(await r.readTextFile("a.txt"), "a");
      assertEquals(r.readTextFileSync("sub/b.txt"), "b");
      assertEquals(
        new TextDecoder().decode(await r.readFile("a.txt")),
        "a",
      );
      assertEquals(new TextDecoder().decode(r.readFileSync("a.txt")), "a");
      await r.writeTextFile("c.txt", "c");
      r.writeTextFileSync("sub/d.txt", "d");
      await r.writeFile("e.bin", new Uint8Array([1, 2]));
      r.writeFileSync("f.bin", new Uint8Array([3]));
      assertEquals(Deno.readTextFileSync(`${root}/c.txt`), "c");
      assertEquals(Deno.readTextFileSync(`${root}/sub/d.txt`), "d");
      assertEquals(Deno.readFileSync(`${root}/e.bin`), new Uint8Array([1, 2]));
      assertEquals(Deno.readFileSync(`${root}/f.bin`), new Uint8Array([3]));
      await r.writeTextFile("c.txt", "+", { append: true });
      assertEquals(Deno.readTextFileSync(`${root}/c.txt`), "c+");
      await assertRejects(
        () => r.writeTextFile("c.txt", "x", { createNew: true }),
        Deno.errors.AlreadyExists,
      );
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  function openRootSyncDotDotInsideIsAllowed() {
    const { root } = setup();
    const r = Deno.openRootSync(root);
    try {
      assertEquals(r.readTextFileSync("sub/../a.txt"), "a");
      assertEquals(r.readTextFileSync("./sub/./b.txt"), "b");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootWritesStreams() {
    const { root } = setup();
    const r = await Deno.openRoot(root, { write: true });
    try {
      await r.writeFile(
        "stream.bin",
        ReadableStream.from([new Uint8Array([1]), new Uint8Array([2])]),
      );
      assertEquals(
        Deno.readFileSync(`${root}/stream.bin`),
        new Uint8Array([1, 2]),
      );
      await r.writeTextFile("stream.txt", ReadableStream.from(["x", "y"]));
      assertEquals(Deno.readTextFileSync(`${root}/stream.txt`), "xy");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootOpenAndCreateReturnFsFiles() {
    const { root } = setup();
    const r = await Deno.openRoot(root, { write: true });
    try {
      using created = await r.create("new.txt");
      assert(created instanceof Deno.FsFile);
      await created.write(new TextEncoder().encode("new"));
      using createdSync = r.createSync("sub/new.txt");
      createdSync.writeSync(new TextEncoder().encode("sub"));
      assertEquals(Deno.readTextFileSync(`${root}/new.txt`), "new");
      assertEquals(Deno.readTextFileSync(`${root}/sub/new.txt`), "sub");

      using opened = await r.open("a.txt");
      const buf = new Uint8Array(1);
      assertEquals(await opened.read(buf), 1);
      assertEquals(new TextDecoder().decode(buf), "a");
      using openedSync = r.openSync("a.txt", { read: true });
      assertEquals(openedSync.statSync().size, 1);

      using writable = r.openSync("a.txt", { write: true, append: true });
      writable.writeSync(new TextEncoder().encode("!"));
      assertEquals(Deno.readTextFileSync(`${root}/a.txt`), "a!");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootDirectoryOperations() {
    const { root } = setup();
    const r = await Deno.openRoot(root, { write: true });
    try {
      await r.mkdir("x");
      r.mkdirSync("y/z", { recursive: true });
      await r.mkdir("p/q/r", { recursive: true });
      await r.mkdir("p/q", { recursive: true });
      assert(Deno.statSync(`${root}/x`).isDirectory);
      assert(Deno.statSync(`${root}/y/z`).isDirectory);
      assert(Deno.statSync(`${root}/p/q/r`).isDirectory);
      await assertRejects(() => r.mkdir("x"), Deno.errors.AlreadyExists);

      const names = [];
      for await (const entry of r.readDir(".")) names.push(entry.name);
      names.sort();
      assertEquals(names, ["a.txt", "p", "sub", "x", "y"]);
      const sync = [...r.readDirSync("sub")];
      assertEquals(sync.length, 1);
      assertEquals(sync[0].name, "b.txt");
      assert(sync[0].isFile);

      await r.rename("a.txt", "x/a.txt");
      r.renameSync("x/a.txt", "a2.txt");
      assertEquals(Deno.readTextFileSync(`${root}/a2.txt`), "a");

      await r.remove("x");
      r.removeSync("a2.txt");
      await assertRejects(() => r.remove("y"), Error);
      await r.remove("y", { recursive: true });
      r.removeSync("p", { recursive: true });
      assertEquals(
        [...Deno.readDirSync(root)].map((e) => e.name).sort(),
        ["sub"],
      );
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootStatAndLstat() {
    const { root } = setup();
    const r = Deno.openRootSync(root);
    try {
      const info = await r.stat("a.txt");
      assert(info.isFile);
      assertEquals(info.size, 1);
      assert(info.mtime instanceof Date);
      assert(r.statSync("sub").isDirectory);
      assert((await r.lstat("sub")).isDirectory);
      assertEquals(r.lstatSync("sub/b.txt").size, 1);
      await assertRejects(() => r.stat("missing"), Deno.errors.NotFound);
      assertThrows(() => r.lstatSync("missing"), Deno.errors.NotFound);
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootRejectsEscapes() {
    const { root, outside } = setup();
    const r = await Deno.openRoot(root, { write: true });
    try {
      for (const name of ["../outside.txt", "sub/../../outside.txt", outside]) {
        await assertRejects(
          () => r.readTextFile(name),
          Deno.errors.PathEscape,
          "escapes from its root",
        );
        assertThrows(() => r.readFileSync(name), Deno.errors.PathEscape);
        await assertRejects(() => r.open(name), Deno.errors.PathEscape);
        assertThrows(() => r.createSync(name), Deno.errors.PathEscape);
        await assertRejects(
          () => r.writeTextFile(name, "x"),
          Deno.errors.PathEscape,
        );
        assertThrows(
          () => r.writeFileSync(name, new Uint8Array()),
          Deno.errors.PathEscape,
        );
        await assertRejects(() => r.stat(name), Deno.errors.PathEscape);
        assertThrows(() => r.lstatSync(name), Deno.errors.PathEscape);
        await assertRejects(() => r.remove(name), Deno.errors.PathEscape);
        assertThrows(() => r.mkdirSync(name), Deno.errors.PathEscape);
        await assertRejects(
          () => r.rename("a.txt", name),
          Deno.errors.PathEscape,
        );
        assertThrows(() => r.renameSync(name, "a.txt"), Deno.errors.PathEscape);
      }
      await assertRejects(
        async () => {
          for await (const _ of r.readDir("..")) {
            // unreachable
          }
        },
        Deno.errors.PathEscape,
      );
      assertThrows(() => r.readDirSync(".."), Deno.errors.PathEscape);
      await assertRejects(() => r.openRoot(".."), Deno.errors.PathEscape);
      assertThrows(() => r.openRootSync(".."), Deno.errors.PathEscape);
      await assertRejects(
        () => r.mkdir("../made", { recursive: true }),
        Deno.errors.PathEscape,
      );
      assertEquals(Deno.readTextFileSync(outside), "outside");
      assertEquals(Deno.readTextFileSync(`${root}/a.txt`), "a");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  function openRootEscapeMessageNamesThePath() {
    const { root } = setup();
    const r = Deno.openRootSync(root);
    try {
      const err = assertThrows(
        () => r.readTextFileSync("../outside.txt"),
        Deno.errors.PathEscape,
      );
      assert(
        err.message.includes("../outside.txt") ||
          err.message.includes("..\\outside.txt"),
        err.message,
      );
      assertEquals(err.name, "PathEscape");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { ignore: isWindows, permissions: { read: true, write: true } },
  async function openRootSymlinks() {
    const { root, outside } = setup();
    Deno.symlinkSync(outside, `${root}/out-abs`);
    Deno.symlinkSync("../outside.txt", `${root}/out-rel`);
    Deno.symlinkSync("a.txt", `${root}/in`);
    Deno.symlinkSync("../a.txt", `${root}/sub/up`);
    Deno.symlinkSync("..", `${root}/sub/parent`);
    const r = await Deno.openRoot(root, { write: true });
    try {
      // Reads follow a symlink while it stays inside.
      assertEquals(await r.readTextFile("in"), "a");
      assertEquals(r.readTextFileSync("sub/up"), "a");
      assertEquals(r.readTextFileSync("sub/parent/a.txt"), "a");
      assert(r.statSync("in").isFile);
      assert((await r.lstat("in")).isSymlink);
      using f = r.openSync("in");
      assertEquals(f.statSync().size, 1);

      // ... and refuse one that points out.
      for (const name of ["out-abs", "out-rel"]) {
        await assertRejects(() => r.readTextFile(name), Deno.errors.PathEscape);
        assertThrows(() => r.statSync(name), Deno.errors.PathEscape);
        assertThrows(() => r.openSync(name), Deno.errors.PathEscape);
      }

      // Writes don't follow a final symlink, even one inside.
      await assertRejects(() => r.writeTextFile("in", "x"), Error);
      assertThrows(() => r.writeTextFileSync("out-rel", "x"), Error);
      assertEquals(Deno.readTextFileSync(`${root}/a.txt`), "a");
      assertEquals(Deno.readTextFileSync(outside), "outside");

      // Removing a link removes the link, not its target.
      await r.remove("out-abs");
      assertEquals(Deno.readTextFileSync(outside), "outside");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { ignore: isWindows, permissions: { read: true, write: true } },
  function openRootIntermediateDirectorySwappedForSymlink() {
    const { dir, root } = setup();
    const r = Deno.openRootSync(root);
    try {
      Deno.removeSync(`${root}/sub`, { recursive: true });
      Deno.symlinkSync(dir, `${root}/sub`);
      assertThrows(
        () => r.readTextFileSync("sub/outside.txt"),
        Deno.errors.PathEscape,
      );
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootSubRootCannotReachItsParent() {
    const { root } = setup();
    const r = await Deno.openRoot(root, { write: true });
    const sub = await r.openRoot("sub");
    const subSync = r.openRootSync("sub");
    try {
      assertEquals(await sub.readTextFile("b.txt"), "b");
      await sub.writeTextFile("c.txt", "c");
      assertEquals(Deno.readTextFileSync(`${root}/sub/c.txt`), "c");
      assertThrows(
        () => subSync.readTextFileSync("../a.txt"),
        Deno.errors.PathEscape,
      );
    } finally {
      sub.close();
      subSync.close();
      r.close();
    }
  },
);

// On Windows the directory can't be renamed while a root holds it open.
Deno.test(
  { ignore: isWindows, permissions: { read: true, write: true } },
  async function openRootFollowsTheDirectoryNotThePath() {
    const { dir, root } = setup();
    const r = await Deno.openRoot(root);
    try {
      Deno.renameSync(root, `${dir}/moved`);
      assertEquals(await r.readTextFile("a.txt"), "a");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootReadOnlyByDefault() {
    const { root } = setup();
    const r = await Deno.openRoot(root);
    try {
      await assertRejects(
        () => r.writeTextFile("x.txt", "x"),
        Deno.errors.NotCapable,
        "write: true",
      );
      assertThrows(() => r.createSync("x.txt"), Deno.errors.NotCapable);
      assertThrows(() => r.mkdirSync("x"), Deno.errors.NotCapable);
      assertThrows(() => r.removeSync("a.txt"), Deno.errors.NotCapable);
      assertThrows(
        () => r.renameSync("a.txt", "b.txt"),
        Deno.errors.NotCapable,
      );
      assertThrows(
        () => r.openSync("a.txt", { write: true }),
        Deno.errors.NotCapable,
      );
      const sub = r.openRootSync("sub");
      try {
        assertThrows(
          () => sub.writeTextFileSync("x.txt", "x"),
          Deno.errors.NotCapable,
        );
      } finally {
        sub.close();
      }
      assertEquals([...Deno.readDirSync(root)].length, 2);
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootClosedThrowsBadResource() {
    const { root } = setup();
    const r = await Deno.openRoot(root);
    r.close();
    assertThrows(() => r.readTextFileSync("a.txt"), Deno.errors.BadResource);
    await assertRejects(() => r.stat("a.txt"), Deno.errors.BadResource);
    assertThrows(() => r.close(), Deno.errors.BadResource);
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootDisposal() {
    const { root } = setup();
    let kept: Deno.Root;
    {
      using r = Deno.openRootSync(root);
      kept = r;
      assertEquals(r.readTextFileSync("a.txt"), "a");
    }
    assertThrows(() => kept.readTextFileSync("a.txt"), Deno.errors.BadResource);
    {
      await using r = await Deno.openRoot(root);
      kept = r;
      assertEquals(await r.readTextFile("a.txt"), "a");
    }
    await assertRejects(
      () => kept.readTextFile("a.txt"),
      Deno.errors.BadResource,
    );
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  async function openRootErrors() {
    const { dir, root } = setup();
    await assertRejects(
      () => Deno.openRoot(`${dir}/missing`),
      Deno.errors.NotFound,
    );
    assertThrows(() => Deno.openRootSync(`${root}/a.txt`), Error);
    const r = Deno.openRootSync(root);
    try {
      await assertRejects(
        () => r.readTextFile("missing"),
        Deno.errors.NotFound,
      );
      const controller = new AbortController();
      controller.abort();
      await assertRejects(
        () => r.readFile("a.txt", { signal: controller.signal }),
        DOMException,
      );
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  function openRootAcceptsUrlsForTheRootOnly() {
    const { root } = setup();
    const r = Deno.openRootSync(
      new URL(`file://${isWindows ? "/" : ""}${root}`),
    );
    try {
      assertEquals(r.readTextFileSync("a.txt"), "a");
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { ignore: !isWindows, permissions: { read: true, write: true } },
  function openRootRejectsWindowsDeviceNames() {
    const { root } = setup();
    const r = Deno.openRootSync(root, { write: true });
    try {
      for (const name of ["NUL", "COM1.txt", "sub/LPT1"]) {
        assertThrows(() => r.readTextFileSync(name), Deno.errors.PathEscape);
        assertThrows(
          () => r.writeTextFileSync(name, "x"),
          Deno.errors.PathEscape,
        );
      }
    } finally {
      r.close();
    }
  },
);

Deno.test(
  { ignore: isWindows, permissions: { read: true, write: true } },
  async function rootStatOfAFileThatCannotBeOpened() {
    // stat needs no read access to the file itself, as with Deno.stat().
    const { root } = setup();
    Deno.writeTextFileSync(`${root}/m000`, "x");
    Deno.chmodSync(`${root}/m000`, 0o000);
    using r = Deno.openRootSync(root);
    try {
      assert(r.statSync("m000").isFile);
      assertEquals((await r.stat("m000")).size, 1);
    } finally {
      Deno.chmodSync(`${root}/m000`, 0o644);
    }
  },
);

Deno.test({ permissions: { read: false } }, async function openRootNeedsRead() {
  await assertRejects(() => Deno.openRoot("."), Deno.errors.NotCapable);
  assertThrows(() => Deno.openRootSync("."), Deno.errors.NotCapable);
});

Deno.test(
  { permissions: { read: true, write: false } },
  async function openRootWriteNeedsWrite() {
    await assertRejects(
      () => Deno.openRoot(".", { write: true }),
      Deno.errors.NotCapable,
    );
  },
);

Deno.test(
  { permissions: { read: true, write: true } },
  function rootCannotBeConstructed() {
    assertThrows(
      () => {
        // deno-lint-ignore no-explicit-any
        new (Deno.Root as any)(1);
      },
      TypeError,
      "cannot be constructed",
    );
  },
);
