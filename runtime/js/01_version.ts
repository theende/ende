// Copyright 2018-2026 the Deno authors. MIT license.

(function () {
const { primordials } = __bootstrap;
const {
  ObjectFreeze,
} = primordials;

const version = {
  ende: "",
  v8: "",
  typescript: "",
};

function setVersions(
  denoVersion,
  v8Version,
  tsVersion,
) {
  version.ende = denoVersion;
  version.v8 = v8Version;
  version.typescript = tsVersion;

  ObjectFreeze(version);
}

return { setVersions, version };
})();
