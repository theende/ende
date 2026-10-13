# Checkin: A Tiny Testing Tool for `ende_core`

<img src="docs/logo.png" align="right" width="100" />

## Why Checkin?

Dino : _Deno_ :: Chicken : _Checkin_

Also because it _checks_ how ende_core works. Yuk yuk.

## Overview

_Checkin_ is a tiny testing tool designed to exercise the functionality of
`ende_core`. It implements a very small standard library which is just enough to
exercise all of the components that we expose to
[Ende](https://github.com/theende/ende) without reinventing the wheel.

## Modules

Modules in _Checkin_ are written in TypeScript and transpiled before loading.
