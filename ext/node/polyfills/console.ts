// Copyright 2018-2026 the Deno authors. MIT license.

(function () {
const { core } = __bootstrap;
const { Console } = core.loadExtScript(
  "ext:ende_node/internal/console/constructor.mjs",
);

return { Console };
})();
