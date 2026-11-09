// Copyright 2026 the Ende authors. MIT license.

// The executable of a macOS `ende desktop --compress` bundle. See README.md.
//
// The thin bundle's Info.plist names what to do:
//   EndeSelfExtractPayload     archive in Contents/Resources (payload.tar.xz)
//   EndeSelfExtractHash        cache key of the payload
//   EndeSelfExtractApp         the real bundle inside the archive (MyApp.app)
//   EndeSelfExtractExecutable  that bundle's CFBundleExecutable
//
// The launcher extracts the payload to
// `$HOME/Library/Application Support/<CFBundleIdentifier>/<hash>/` unless the
// real bundle's executable is already there, then asks LaunchServices to open
// the real bundle with the launcher's arguments, and exits. It doesn't exec
// the real executable: LaunchServices registers the process it launched, and
// after an exec that registration is gone, so the app's NSStatusItems
// (Ende.Tray) never reach the menu bar (denoland/deno#35619). Opened through
// LaunchServices, the real bundle runs as an app of its own, and opening the
// thin bundle again brings an already running copy to the front.

#include <CoreFoundation/CoreFoundation.h>
#include <errno.h>
#include <limits.h>
#include <pwd.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static int fail(const char *message, const char *detail) {
  fprintf(stderr, "ende launcher: %s: %s\n", message, detail);
  return 1;
}

// Copies the main bundle's Info.plist string `key` into `buf`. Rejects
// missing values and anything that could leave the directory it's joined to.
static int info_string(CFBundleRef bundle, const char *key, char *buf,
                       size_t len) {
  CFStringRef cf_key =
      CFStringCreateWithCString(NULL, key, kCFStringEncodingUTF8);
  CFTypeRef value = CFBundleGetValueForInfoDictionaryKey(bundle, cf_key);
  CFRelease(cf_key);
  if (value == NULL || CFGetTypeID(value) != CFStringGetTypeID() ||
      !CFStringGetCString(value, buf, (CFIndex)len, kCFStringEncodingUTF8) ||
      buf[0] == '\0' || buf[0] == '.' || strchr(buf, '/') != NULL) {
    return fail("missing or invalid Info.plist key", key);
  }
  return 0;
}

// Formats into `buf`, failing if the result doesn't fit.
static int join(char *buf, size_t len, const char *format, const char *a,
                const char *b, const char *c) {
  int n = snprintf(buf, len, format, a, b, c);
  if (n < 0 || (size_t)n >= len) {
    return fail("path too long", a);
  }
  return 0;
}

// `mkdir -p`.
static int make_dirs(const char *path) {
  char buf[PATH_MAX];
  if (strlcpy(buf, path, sizeof buf) >= sizeof buf) {
    return fail("path too long", path);
  }
  for (char *p = buf + 1;; p++) {
    if (*p == '/' || *p == '\0') {
      char saved = *p;
      *p = '\0';
      if (mkdir(buf, 0755) != 0 && errno != EEXIST) {
        return fail(strerror(errno), buf);
      }
      *p = saved;
      if (saved == '\0') {
        return 0;
      }
    }
  }
}

// Runs `argv` with this process's environment and returns its exit status.
static int run(char *const argv[]) {
  pid_t pid;
  int err = posix_spawn(&pid, argv[0], NULL, NULL, argv, environ);
  if (err != 0) {
    return fail(strerror(err), argv[0]);
  }
  int status;
  while (waitpid(pid, &status, 0) < 0) {
    if (errno != EINTR) {
      return fail(strerror(errno), argv[0]);
    }
  }
  return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}

int main(int argc, char *argv[]) {
  CFBundleRef bundle = CFBundleGetMainBundle();
  char bundle_path[PATH_MAX];
  CFURLRef bundle_url = bundle ? CFBundleCopyBundleURL(bundle) : NULL;
  if (bundle_url == NULL ||
      !CFURLGetFileSystemRepresentation(bundle_url, true,
                                        (UInt8 *)bundle_path,
                                        sizeof bundle_path)) {
    return fail("can't find the app bundle", argv[0]);
  }
  CFRelease(bundle_url);

  char id[256], hash[256], payload[256], app[256], exe[256];
  if (info_string(bundle, "CFBundleIdentifier", id, sizeof id) ||
      info_string(bundle, "EndeSelfExtractHash", hash, sizeof hash) ||
      info_string(bundle, "EndeSelfExtractPayload", payload, sizeof payload) ||
      info_string(bundle, "EndeSelfExtractApp", app, sizeof app) ||
      info_string(bundle, "EndeSelfExtractExecutable", exe, sizeof exe)) {
    return 1;
  }

  const char *home = getenv("HOME");
  if (home == NULL || home[0] == '\0') {
    struct passwd *pw = getpwuid(getuid());
    home = pw ? pw->pw_dir : NULL;
  }
  if (home == NULL) {
    return fail("can't find the home directory", "HOME");
  }

  char dest[PATH_MAX], inner[PATH_MAX], inner_exe[PATH_MAX];
  char payload_path[PATH_MAX];
  if (join(dest, sizeof dest, "%s/Library/Application Support/%s/%s", home,
           id, hash) ||
      join(inner, sizeof inner, "%s/%s%s", dest, app, "") ||
      join(inner_exe, sizeof inner_exe, "%s/Contents/MacOS/%s%s", inner, exe,
           "") ||
      join(payload_path, sizeof payload_path, "%s/Contents/Resources/%s%s",
           bundle_path, payload, "")) {
    return 1;
  }

  if (access(inner_exe, X_OK) != 0) {
    if (make_dirs(dest) != 0) {
      return 1;
    }
    char *tar[] = {"/usr/bin/tar", "-xf", payload_path, "-C", dest, NULL};
    if (run(tar) != 0) {
      return fail("can't extract", payload_path);
    }
  }

  // open <inner> [--args <arguments>...]
  char **open_argv = calloc((size_t)argc + 3, sizeof(char *));
  if (open_argv == NULL) {
    return fail(strerror(errno), "calloc");
  }
  int n = 0;
  open_argv[n++] = "/usr/bin/open";
  open_argv[n++] = inner;
  if (argc > 1) {
    open_argv[n++] = "--args";
    for (int i = 1; i < argc; i++) {
      open_argv[n++] = argv[i];
    }
  }
  open_argv[n] = NULL;
  return run(open_argv);
}
