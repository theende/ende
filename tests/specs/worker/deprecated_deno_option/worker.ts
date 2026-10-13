// Copyright 2026 the Ende authors. MIT license.
const { state } = Ende.permissions.querySync({ name: "read" });
postMessage(state === "granted");
close();
