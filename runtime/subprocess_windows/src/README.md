# deno_subprocess_windows

A library for spawning subprocesses on windows, with support for detached
processes and passing file descriptors down to child processes.

The code is largely adapted from libuv (line for line in some cases, including
comments), with some parts adapted from the rust std library.

The interface mimics `std::process::Command`, though some types (e.g. `Stdio`)
aren't compatible with the std version.
