# ddevmem-macros

Proc-macro companion crate for [`ddevmem`].

This crate is **not intended to be used directly**. It only exposes the
`register_map!` macro, which is re-exported by `ddevmem` when the
`register-map` feature is enabled. Add `ddevmem` to your dependencies
instead:

```toml
[dependencies]
ddevmem = "0.4"
```

See the [`ddevmem` crate documentation][docs] for usage.

[`ddevmem`]: https://crates.io/crates/ddevmem
[docs]: https://docs.rs/ddevmem
