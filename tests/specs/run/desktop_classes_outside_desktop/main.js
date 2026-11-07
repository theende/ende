const ops = Ende[Ende.internal].core.ops;
for (const name of ["BrowserWindow", "Tray", "Dock", "Notification"]) {
  try {
    new ops[name](...(name === "Notification" ? ["title"] : []));
    console.log(name, "constructed");
  } catch (e) {
    console.log(name, "threw", e.name + ":", e.message);
  }
}

const worker = new Worker(
  URL.createObjectURL(
    new Blob([
      `const ops = Ende[Ende.internal].core.ops;
      try {
        new ops.BrowserWindow();
        postMessage("constructed");
      } catch (e) {
        postMessage("threw " + e.name + ": " + e.message);
      }`,
    ], { type: "application/javascript" }),
  ),
  { type: "module" },
);
worker.onmessage = (e) => {
  console.log("BrowserWindow in a Worker", e.data);
  worker.terminate();
};
