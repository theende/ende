# ende_io

**This crate provides IO primitives for other Ende extensions, this includes
stdio streams and abstraction over File System files.**

## Usage Example

From javascript, include the extension's source:

```javascript
import { core } from "ext:core/mod.js";

const io = core.loadExtScript("ext:ende_io/12_io.js");
```

Then from rust, provide: `ende_io::ende_io::init(Option<ende_io::Stdio>)` in the
`extensions` field of your `RuntimeOptions`

Where `ende_io::Stdio` implements `Default`, and can therefore be provided as
`Some(ende_io::Stdio::default())`

## Dependencies

- **ende_web**: Provided by the `ende_web` crate
- **ende_tty**: Provided in `runtime/ops/tty.rs`
