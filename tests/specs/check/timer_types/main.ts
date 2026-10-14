// Copyright 2026 the Ende authors. MIT license.
// setTimeout/setInterval return numeric ids, like in browsers, and
// clearTimeout/clearInterval and Ende.refTimer/unrefTimer take them.

const timeout: number = setTimeout(() => {}, 1);
clearTimeout(timeout);

const interval: number = setInterval(() => {}, 1000);
Ende.unrefTimer(interval);
Ende.refTimer(interval);
clearInterval(interval);

const withArgs: number = setTimeout((a: string, b: number) => {}, 1, "a", 1);
clearTimeout(withArgs);
